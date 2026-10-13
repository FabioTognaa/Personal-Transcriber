use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Instant, UNIX_EPOCH};

use crossbeam_channel::{Sender, bounded};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::domain::{
    InferenceConfig, InferenceStrategy, ModelIdentity, SpeechSegment, TranscriptSegment,
};
use crate::schema::SCHEMA_VERSION;
use crate::session::{SegmentSink, SegmentSinkStatus};
use crate::storage::{TranscriptWriter, atomic_write_json, read_json};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct AsrOutput {
    pub text: String,
    pub inference_duration_us: u64,
    /// Worst `no_speech_probability` whisper reported for the segment: the
    /// confidence ruler used to gate prompt context and inspect quality.
    pub no_speech_probability: Option<f32>,
}

pub trait AsrEngine: Send + 'static {
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        language: &str,
        config: &InferenceConfig,
    ) -> Result<AsrOutput>;
}

const ASR_QUEUE_CAPACITY: usize = 8;

/// Above this no-speech probability the previous segment is considered
/// unreliable and its text is not offered as context (whisper's own default
/// no-speech threshold).
pub const CONTEXT_MAX_NO_SPEECH_PROBABILITY: f32 = 0.6;

/// Effective prompt for the next segment when `--asr-context` is on: the static
/// language prompt plus a short tail of the previous segment's text. The gate
/// skips the tail when the previous segment was low-confidence, so a doubtful
/// guess cannot repeat in later segments. The dynamic prompt itself is never
/// recorded: it is reconstructible from the transcript.
#[must_use]
pub fn contextual_prompt(
    config: &InferenceConfig,
    previous_text: &str,
    previous_no_speech_probability: Option<f32>,
) -> Option<String> {
    if !config.context_enabled
        || previous_no_speech_probability
            .is_some_and(|probability| probability > CONTEXT_MAX_NO_SPEECH_PROBABILITY)
    {
        return config.prompt.clone();
    }
    let base = config.prompt.clone();
    let tail = prompt_tail(previous_text, config.context_window_chars);
    match (base, tail) {
        (Some(base), Some(tail)) => Some(format!("{base} {tail}")),
        (prompt, None) => prompt,
        (None, prompt) => prompt,
    }
}

/// Last `window_chars` characters of `text`, trimmed; `None` when empty.
fn prompt_tail(text: &str, window_chars: usize) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || window_chars == 0 {
        return None;
    }
    let skip = trimmed.chars().count().saturating_sub(window_chars);
    let tail: String = trimmed.chars().skip(skip).collect();
    let tail = tail.trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_owned())
    }
}

/// Built-in initial prompt for a supported language.
///
/// The prompt is a short, natural preamble that biases Whisper toward correct
/// punctuation, capitalization and accents. It is deliberately generic so it
/// does not inject meeting-specific content. Users can override it with
/// `--asr-prompt` to add a domain glossary (names, acronyms, product terms).
#[must_use]
pub fn default_initial_prompt(language: &str) -> Option<&'static str> {
    match language {
        "it" => Some(
            "Trascrizione in italiano di una riunione. Usa una punteggiatura corretta, le maiuscole appropriate e gli accenti.",
        ),
        "en" => {
            Some("Transcript in English of a meeting. Use correct punctuation and capitalization.")
        }
        _ => None,
    }
}

enum AsrCommand {
    Segment(SpeechSegment),
    Shutdown,
}

pub struct TranscribingSink<E: AsrEngine> {
    engine: Option<E>,
    model: ModelIdentity,
    language: String,
    inference: InferenceConfig,
    sender: Option<Sender<AsrCommand>>,
    worker: Option<JoinHandle<Result<()>>>,
    status: Arc<Mutex<SegmentSinkStatus>>,
}

impl<E: AsrEngine> TranscribingSink<E> {
    #[must_use]
    pub fn new(
        engine: E,
        model: ModelIdentity,
        language: String,
        inference: InferenceConfig,
    ) -> Self {
        Self {
            engine: Some(engine),
            model,
            language,
            inference,
            sender: None,
            worker: None,
            status: Arc::new(Mutex::new(SegmentSinkStatus {
                queue_capacity: ASR_QUEUE_CAPACITY,
                ..SegmentSinkStatus::default()
            })),
        }
    }
}

impl<E: AsrEngine> SegmentSink for TranscribingSink<E> {
    fn start(&mut self, session_id: Uuid, transcript_path: &Path) -> Result<()> {
        let (sender, receiver) = bounded(ASR_QUEUE_CAPACITY);
        let mut engine = self
            .engine
            .take()
            .ok_or_else(|| Error::Asr("ASR sink cannot be started twice".to_owned()))?;
        let model = self.model.clone();
        let language = self.language.clone();
        let inference = self.inference.clone();
        let transcript_path = transcript_path.to_path_buf();
        let status = Arc::clone(&self.status);
        self.worker = Some(thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut writer = TranscriptWriter::open(&transcript_path)?;
                let mut completed = std::collections::HashSet::new();
                // Previous segment's text and confidence: the mobile prompt
                // source when context is enabled.
                let mut previous: Option<(String, Option<f32>)> = None;
                while let Ok(command) = receiver.recv() {
                    status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .queue_depth = receiver.len();
                    let AsrCommand::Segment(segment) = command else {
                        return Ok(());
                    };
                    if !completed.insert((segment.start, segment.end)) {
                        continue;
                    }
                    let (previous_text, previous_probability) = previous
                        .as_ref()
                        .map(|(text, probability)| (text.as_str(), *probability))
                        .unwrap_or(("", None));
                    let mut call_inference = inference.clone();
                    call_inference.prompt =
                        contextual_prompt(&inference, previous_text, previous_probability);
                    let output = engine.transcribe(&segment, &language, &call_inference)?;
                    let no_speech_probability = output.no_speech_probability;
                    previous = Some((output.text.clone(), no_speech_probability));
                    writer.append(&TranscriptSegment {
                        schema_version: SCHEMA_VERSION,
                        segment_id: Uuid::new_v4(),
                        session_id,
                        start: segment.start,
                        end: segment.end,
                        text: output.text,
                        language: language.clone(),
                        model: model.name.clone(),
                        model_sha256: model.sha256.clone(),
                        // The static config is recorded; the per-segment
                        // dynamic prompt is deliberately not (see
                        // `contextual_prompt`).
                        inference: inference.clone(),
                        no_speech_probability,
                    })?;
                    let mut current = status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    current.metrics.segments_transcribed =
                        current.metrics.segments_transcribed.saturating_add(1);
                    current.metrics.audio_duration_us = current
                        .metrics
                        .audio_duration_us
                        .saturating_add(segment.duration_us());
                    current.metrics.inference_duration_us = current
                        .metrics
                        .inference_duration_us
                        .saturating_add(output.inference_duration_us);
                    current.metrics.last_segment_end = segment.end;
                    current.metrics.real_time_factor_milli = current
                        .metrics
                        .inference_duration_us
                        .saturating_mul(1_000)
                        .checked_div(current.metrics.audio_duration_us)
                        .unwrap_or(0);
                }
                Ok(())
            })();
            if let Err(error) = &result {
                status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .error = Some(error.to_string());
            }
            result
        }));
        self.sender = Some(sender);
        Ok(())
    }

    fn accept(&mut self, segment: &SpeechSegment) -> Result<()> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| Error::Asr("ASR sink has not been started".to_owned()))?;
        sender
            .send(AsrCommand::Segment(segment.clone()))
            .map_err(|_| Error::AsrWorkerPanicked)?;
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.queue_depth = sender.len();
        status.max_queue_depth = status.max_queue_depth.max(status.queue_depth);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        let shutdown_sent = self
            .sender
            .take()
            .is_none_or(|sender| sender.send(AsrCommand::Shutdown).is_ok());
        let worker_result = self
            .worker
            .take()
            .map(|worker| worker.join().map_err(|_| Error::AsrWorkerPanicked))
            .transpose()?;
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue_depth = 0;
        if let Some(result) = worker_result {
            result
        } else if shutdown_sent {
            Ok(())
        } else {
            Err(Error::AsrWorkerPanicked)
        }
    }

    fn status_handle(&self) -> Arc<Mutex<SegmentSinkStatus>> {
        Arc::clone(&self.status)
    }
}

pub struct WhisperEngine {
    context: WhisperContext,
}

impl WhisperEngine {
    pub fn load(model_path: &Path, config: &InferenceConfig) -> Result<Self> {
        validate_inference_config(config)?;
        if config.coreml && !cfg!(feature = "coreml") {
            return Err(Error::InvalidInferenceConfig(
                "Core ML was requested but the binary was built without --features coreml"
                    .to_owned(),
            ));
        }

        let mut parameters = WhisperContextParameters::new();
        parameters.use_gpu(true);
        parameters.flash_attn(config.flash_attention);
        let model_path = model_path
            .to_str()
            .ok_or_else(|| Error::Asr("model path is not valid UTF-8".to_owned()))?;
        let context = WhisperContext::new_with_params(model_path, parameters)
            .map_err(|error| Error::Asr(error.to_string()))?;
        Ok(Self { context })
    }
}

impl AsrEngine for WhisperEngine {
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        language: &str,
        config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        validate_inference_config(config)?;
        let strategy = match config.strategy {
            InferenceStrategy::Greedy => SamplingStrategy::Greedy {
                best_of: config.best_of,
            },
            InferenceStrategy::BeamSearch => SamplingStrategy::BeamSearch {
                beam_size: config.beam_size,
                patience: -1.0,
            },
        };
        let mut parameters = FullParams::new(strategy);
        parameters.set_n_threads(config.threads);
        parameters.set_translate(false);
        parameters.set_language(Some(language));
        if let Some(prompt) = config.prompt.as_deref().filter(|prompt| !prompt.is_empty()) {
            parameters.set_initial_prompt(prompt);
        }
        parameters.set_temperature(config.temperature);
        // Drop non-speech tag hallucinations like "(musica)"; the decoding
        // thresholds stay at whisper's defaults.
        parameters.set_suppress_nst(true);
        parameters.set_print_special(false);
        parameters.set_print_progress(false);
        parameters.set_print_realtime(false);
        parameters.set_print_timestamps(false);

        let started = Instant::now();
        let mut state = self
            .context
            .create_state()
            .map_err(|error| Error::Asr(error.to_string()))?;
        state
            .full(parameters, &segment.samples)
            .map_err(|error| Error::Asr(error.to_string()))?;

        let segment_count = state.full_n_segments();
        let mut text = String::new();
        let mut no_speech_probability: Option<f32> = None;
        for index in 0..segment_count {
            let whisper_segment = state.get_segment(index).ok_or_else(|| {
                Error::Asr("whisper returned an invalid segment index".to_owned())
            })?;
            let probability = whisper_segment.no_speech_probability();
            no_speech_probability =
                Some(no_speech_probability.map_or(probability, |worst| worst.max(probability)));
            text.push_str(
                whisper_segment
                    .to_str()
                    .map_err(|error| Error::Asr(error.to_string()))?,
            );
        }
        Ok(AsrOutput {
            text,
            inference_duration_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            no_speech_probability,
        })
    }
}

/// SHA-256 of a file, read in bounded chunks so large files are never loaded
/// fully into memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Serialize, Deserialize)]
struct CachedModelIdentity {
    size_bytes: u64,
    modified_nanos: u64,
    sha256: String,
}

/// SHA-256 of `path`, reusing a `<name>.sha256` sidecar when the size and
/// modification time are unchanged, so multi-gigabyte models are not re-hashed
/// on every `start`. Best effort: a missing or unreadable cache just means the
/// file is hashed again.
fn cached_sha256(path: &Path, metadata: &std::fs::Metadata) -> Result<String> {
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok());
    let cache_path = path.with_extension("sha256");
    if let Some(modified_nanos) = modified_nanos
        && let Ok(cached) = read_json::<CachedModelIdentity>(&cache_path)
        && cached.size_bytes == metadata.len()
        && cached.modified_nanos == modified_nanos
    {
        return Ok(cached.sha256);
    }
    let sha256 = sha256_file(path)?;
    if let Some(modified_nanos) = modified_nanos {
        let _ = atomic_write_json(
            &cache_path,
            &CachedModelIdentity {
                size_bytes: metadata.len(),
                modified_nanos,
                sha256: sha256.clone(),
            },
        );
    }
    Ok(sha256)
}

pub fn model_identity(path: &Path) -> Result<ModelIdentity> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(Error::Asr(format!(
            "model is not a non-empty file: {}",
            path.display()
        )));
    }

    let sha256 = cached_sha256(path, &metadata)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Asr("model filename is not valid UTF-8".to_owned()))?
        .to_owned();

    Ok(ModelIdentity {
        path: path.to_path_buf(),
        name,
        sha256,
        size_bytes: metadata.len(),
    })
}

pub fn validate_inference_config(config: &InferenceConfig) -> Result<()> {
    if config.threads <= 0 {
        return Err(Error::InvalidInferenceConfig(
            "thread count must be greater than zero".to_owned(),
        ));
    }
    if config.best_of <= 0 || config.beam_size <= 0 {
        return Err(Error::InvalidInferenceConfig(
            "best-of and beam size must be greater than zero".to_owned(),
        ));
    }
    if !config.temperature.is_finite() || config.temperature < 0.0 {
        return Err(Error::InvalidInferenceConfig(
            "temperature must be finite and non-negative".to_owned(),
        ));
    }
    if let Some(prompt) = config.prompt.as_deref()
        && prompt.contains('\0')
    {
        return Err(Error::InvalidInferenceConfig(
            "initial prompt must not contain a null byte".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_identity_reuses_the_sidecar_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let model = temporary.path().join("model.bin");
        std::fs::write(&model, b"some model bytes").expect("model fixture should be written");

        let first = model_identity(&model).expect("identity should be computed");
        let cache_path = model.with_extension("sha256");
        let mut cached: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&cache_path).expect("cache sidecar should exist"),
        )
        .expect("cache should be valid JSON");
        cached["sha256"] = serde_json::Value::String("cached-value".to_owned());
        std::fs::write(
            &cache_path,
            serde_json::to_vec(&cached).expect("cache should serialize"),
        )
        .expect("cache should be rewritten");

        let second = model_identity(&model).expect("identity should come from the cache");

        assert_eq!(first.sha256.len(), 64);
        assert_eq!(second.sha256, "cached-value");
    }

    #[test]
    fn contextual_prompt_combines_gates_and_windows_the_tail() {
        let config = InferenceConfig {
            prompt: Some("base".to_owned()),
            context_enabled: true,
            context_window_chars: 3,
            ..InferenceConfig::default()
        };

        assert_eq!(
            contextual_prompt(&config, "abc def ghi ", Some(0.1)),
            Some("base ghi".to_owned())
        );
        // Gate: a low-confidence previous segment keeps only the static prompt.
        assert_eq!(
            contextual_prompt(&config, "abc def ghi", Some(0.9)),
            Some("base".to_owned())
        );
        // No previous text yet: nothing to carry over.
        assert_eq!(
            contextual_prompt(&config, "", None),
            Some("base".to_owned())
        );
    }

    #[test]
    fn contextual_prompt_off_leaves_the_prompt_untouched() {
        let config = InferenceConfig {
            prompt: Some("base".to_owned()),
            context_enabled: false,
            ..InferenceConfig::default()
        };

        assert_eq!(
            contextual_prompt(&config, "abc def ghi", Some(0.1)),
            Some("base".to_owned())
        );
    }
}
