use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Instant, UNIX_EPOCH};

use crossbeam_channel::{Sender, bounded};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::domain::{
    InferenceConfig, InferenceStrategy, ModelIdentity, SpeechSegment, TimedText, TimestampUs,
    TranscriptSegment, samples_to_us,
};
use crate::model::{ArtifactKind, ModelArtifact};
use crate::schema::SCHEMA_VERSION;
use crate::session::{SegmentSink, SegmentSinkStatus};
use crate::storage::{TranscriptWriter, atomic_write_json, read_json};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrOutput {
    pub text: String,
    pub inference_duration_us: u64,
}

/// Progress reporter shared with the engines. It is `Clone + 'static` so it can
/// be captured by whisper.cpp's safe progress callback.
type ProgressCallback = Box<dyn FnMut(u64) + Send>;

#[derive(Clone)]
pub struct Progress(Arc<Mutex<ProgressCallback>>);

impl Progress {
    pub fn new<F>(callback: F) -> Self
    where
        F: FnMut(u64) + Send + 'static,
    {
        Self(Arc::new(Mutex::new(Box::new(callback))))
    }

    pub fn report(&self, transcribed_us: u64) {
        let mut callback = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        callback(transcribed_us);
    }
}

/// Maximum duration of one `.txt` line after merging whisper's micro-segments.
pub const MERGE_MAX_LINE_DURATION_US: u64 = 20_000_000;
/// A silence at least this long closes the current line.
const MERGE_GAP_US: u64 = 500_000;

/// Whisper returns micro-segments of about one second. The live path used the VAD
/// to group them; the batch path groups them here, so the `.txt` stays readable
/// without losing the native timestamps. The canonical JSONL stores the merged
/// pieces too, which keeps `export --format text` byte-identical to the `.txt`
/// without its prefixes.
pub fn merge_timed_text(pieces: Vec<TimedText>) -> Vec<TimedText> {
    let mut merged: Vec<TimedText> = Vec::new();
    for piece in pieces {
        let Some(current) = merged.last_mut() else {
            merged.push(piece);
            continue;
        };
        let gap_us = piece.start.0.saturating_sub(current.end.0);
        let would_end_us = piece.end.0.saturating_sub(current.start.0);
        if ends_sentence(&current.text)
            || gap_us >= MERGE_GAP_US
            || would_end_us > MERGE_MAX_LINE_DURATION_US
        {
            merged.push(piece);
        } else {
            current.end = piece.end;
            if !current.text.is_empty() && !piece.text.is_empty() {
                current.text.push(' ');
            }
            current.text.push_str(&piece.text);
        }
    }
    merged
}

fn ends_sentence(text: &str) -> bool {
    let trimmed = text.trim_end().trim_end_matches(['"', '”', '’', '\'']);
    matches!(trimmed.chars().last(), Some('.' | '!' | '?' | '…'))
}

pub trait AsrEngine: Send + 'static {
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        language: &str,
        config: &InferenceConfig,
    ) -> Result<AsrOutput>;

    /// Transcribes the whole file in one batch call and returns timed pieces.
    ///
    /// The default implementation wraps the live path and is meant for mocks;
    /// real engines override it so they can preserve their native segmentation.
    fn transcribe_batch(
        &mut self,
        samples: &[f32],
        language: &str,
        config: &InferenceConfig,
        _progress: &Progress,
    ) -> Result<Vec<TimedText>> {
        let end = TimestampUs(samples_to_us(samples.len()));
        let output = self.transcribe(
            &SpeechSegment {
                start: TimestampUs(0),
                end,
                samples: samples.to_vec(),
            },
            language,
            config,
        )?;
        let text = output.text.trim().to_owned();
        Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![TimedText {
                start: TimestampUs(0),
                end,
                text,
            }]
        })
    }
}

impl AsrEngine for Box<dyn AsrEngine> {
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        language: &str,
        config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        (**self).transcribe(segment, language, config)
    }

    fn transcribe_batch(
        &mut self,
        samples: &[f32],
        language: &str,
        config: &InferenceConfig,
        progress: &Progress,
    ) -> Result<Vec<TimedText>> {
        (**self).transcribe_batch(samples, language, config, progress)
    }
}

/// Build the engine that runs `artifact`. One engine per process: the caller
/// drops the previous one before loading another.
pub fn load_engine(
    artifact: &ModelArtifact,
    config: &InferenceConfig,
) -> Result<Box<dyn AsrEngine>> {
    match artifact.kind {
        ArtifactKind::SingleFile => Ok(Box::new(WhisperEngine::load(&artifact.path, config)?)),
        ArtifactKind::Bundle => load_parakeet(artifact, config),
    }
}

#[cfg(feature = "parakeet")]
fn load_parakeet(artifact: &ModelArtifact, config: &InferenceConfig) -> Result<Box<dyn AsrEngine>> {
    Ok(Box::new(crate::parakeet::ParakeetEngine::load(
        artifact, config,
    )?))
}

#[cfg(not(feature = "parakeet"))]
fn load_parakeet(
    _artifact: &ModelArtifact,
    _config: &InferenceConfig,
) -> Result<Box<dyn AsrEngine>> {
    Err(Error::Asr(
        "Parakeet support is not compiled in; rebuild with --features parakeet".to_owned(),
    ))
}

const ASR_QUEUE_CAPACITY: usize = 8;

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
                    let output = engine.transcribe(&segment, &language, &inference)?;
                    if !output.text.trim().is_empty() {
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
                            engine: model.engine,
                            inference: inference.clone(),
                        })?;
                    }
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
        let parameters = whisper_parameters(language, config.prompt.as_deref(), config);

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
        for index in 0..segment_count {
            let whisper_segment = state.get_segment(index).ok_or_else(|| {
                Error::Asr("whisper returned an invalid segment index".to_owned())
            })?;
            text.push_str(
                whisper_segment
                    .to_str()
                    .map_err(|error| Error::Asr(error.to_string()))?,
            );
        }
        Ok(AsrOutput {
            text,
            inference_duration_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        })
    }

    fn transcribe_batch(
        &mut self,
        samples: &[f32],
        language: &str,
        config: &InferenceConfig,
        progress: &Progress,
    ) -> Result<Vec<TimedText>> {
        validate_inference_config(config)?;
        let total_us = samples_to_us(samples.len());
        let mut parameters = whisper_parameters(language, config.prompt.as_deref(), config);
        let callback_progress = progress.clone();
        parameters.set_progress_callback_safe(move |percent: i32| {
            let percent = u64::try_from(percent).unwrap_or(0).min(100);
            callback_progress.report(total_us.saturating_mul(percent) / 100);
        });

        let mut state = self
            .context
            .create_state()
            .map_err(|error| Error::Asr(error.to_string()))?;
        state
            .full(parameters, samples)
            .map_err(|error| Error::Asr(error.to_string()))?;

        let mut segments = Vec::new();
        for index in 0..state.full_n_segments() {
            let whisper_segment = state.get_segment(index).ok_or_else(|| {
                Error::Asr("whisper returned an invalid segment index".to_owned())
            })?;
            let start = TimestampUs(centiseconds_to_us(whisper_segment.start_timestamp()));
            let end = TimestampUs(centiseconds_to_us(whisper_segment.end_timestamp()));
            let text = whisper_segment
                .to_str()
                .map_err(|error| Error::Asr(error.to_string()))?
                .trim()
                .to_owned();
            if !text.is_empty() {
                segments.push(TimedText { start, end, text });
            }
        }
        progress.report(total_us);
        Ok(segments)
    }
}

/// Timestamps returned by whisper.cpp are in centiseconds (10 ms units).
fn centiseconds_to_us(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0).saturating_mul(10_000)
}

fn whisper_parameters<'a, 'b>(
    language: &'a str,
    prompt: Option<&'b str>,
    config: &InferenceConfig,
) -> FullParams<'a, 'b> {
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
    if let Some(prompt) = prompt.filter(|prompt| !prompt.is_empty()) {
        parameters.set_initial_prompt(prompt);
    }
    parameters.set_temperature(config.temperature);
    parameters.set_temperature_inc(config.temperature_inc);
    parameters.set_print_special(false);
    parameters.set_print_progress(false);
    parameters.set_print_realtime(false);
    parameters.set_print_timestamps(false);
    parameters
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

/// SHA-256 identity of an artifact: the file hash for Whisper, the canonical
/// hash of the files that make up the bundle for Parakeet.
pub fn model_identity(artifact: &ModelArtifact) -> Result<ModelIdentity> {
    let (sha256, size_bytes) = match artifact.kind {
        ArtifactKind::SingleFile => {
            let metadata = std::fs::metadata(&artifact.path)?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(Error::Asr(format!(
                    "model is not a non-empty file: {}",
                    artifact.path.display()
                )));
            }
            let sha256 = cached_sha256(&artifact.path, &metadata)?;
            (sha256, metadata.len())
        }
        ArtifactKind::Bundle => bundle_identity(artifact)?,
    };

    Ok(ModelIdentity {
        path: artifact.path.clone(),
        name: artifact.name(),
        sha256,
        size_bytes,
        engine: artifact.kind.engine(),
    })
}

#[derive(Serialize, Deserialize)]
struct CachedBundleIdentity {
    fingerprint: String,
    size_bytes: u64,
    sha256: String,
}

/// Canonical SHA-256 of a bundle: the hash of the sorted `sha256  name` lines of
/// every file that makes up the artifact.
fn bundle_identity(artifact: &ModelArtifact) -> Result<(String, u64)> {
    let files = artifact.files()?;
    let fingerprint = bundle_fingerprint(&files)?;
    let cache_path = bundle_sidecar(&artifact.path);
    if let Ok(cached) = read_json::<CachedBundleIdentity>(&cache_path)
        && cached.fingerprint == fingerprint
    {
        return Ok((cached.sha256, cached.size_bytes));
    }

    let mut size_bytes = 0_u64;
    let mut per_file = Vec::with_capacity(files.len());
    for path in &files {
        let metadata = std::fs::metadata(path)?;
        size_bytes = size_bytes.saturating_add(metadata.len());
        // No per-file sidecar: the bundle-level cache below already avoids
        // re-hashing, and writing inside the model directory would pollute it.
        let sha256 = sha256_file(path)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Asr("model filename is not valid UTF-8".to_owned()))?;
        per_file.push((name.to_owned(), sha256));
    }
    let sha256 = bundle_sha256(&per_file);
    let _ = atomic_write_json(
        &cache_path,
        &CachedBundleIdentity {
            fingerprint,
            size_bytes,
            sha256: sha256.clone(),
        },
    );
    Ok((sha256, size_bytes))
}

/// Fingerprint of the files that make up a bundle, used to reuse the canonical
/// hash when nothing changed.
fn bundle_fingerprint(files: &[PathBuf]) -> Result<String> {
    let mut parts = Vec::with_capacity(files.len());
    for path in files {
        let metadata = std::fs::metadata(path)?;
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
            .unwrap_or(0);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Asr("model filename is not valid UTF-8".to_owned()))?;
        parts.push(format!("{name}:{}:{modified_nanos}", metadata.len()));
    }
    Ok(parts.join("\n"))
}

/// Canonical SHA-256 of a bundle from its `(file name, sha256)` pairs.
///
/// The formula is the hash of the sorted `sha256  name` lines. It is exposed so
/// `doctor` can compute the same aggregate from the per-file hashes it already
/// verifies, without reading the bundle twice.
#[must_use]
pub fn bundle_sha256(files: &[(String, String)]) -> String {
    let mut sorted = files.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let mut canonical = String::new();
    for (name, sha256) in sorted {
        canonical.push_str(&format!("{sha256}  {name}\n"));
    }
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Sidecar next to the bundle directory, e.g. `models/parakeet-....sha256`.
/// `Path::with_extension` cannot be used because bundle names contain dots.
fn bundle_sidecar(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.parent().map_or_else(
        || PathBuf::from(format!("{name}.sha256")),
        |parent| parent.join(format!("{name}.sha256")),
    )
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
    if !config.temperature_inc.is_finite() || config.temperature_inc < 0.0 {
        return Err(Error::InvalidInferenceConfig(
            "temperature increment must be finite and non-negative".to_owned(),
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
    use crate::domain::TimestampUs;
    use crate::session::SegmentSink;

    #[test]
    fn model_identity_reuses_the_sidecar_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let model = temporary.path().join("model.bin");
        std::fs::write(&model, b"some model bytes").expect("model fixture should be written");

        let first = model_identity(&ModelArtifact::detect(&model).expect("artifact"))
            .expect("identity should be computed");
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

        let second = model_identity(&ModelArtifact::detect(&model).expect("artifact"))
            .expect("identity should come from the cache");

        assert_eq!(first.sha256.len(), 64);
        assert_eq!(second.sha256, "cached-value");
    }

    #[test]
    fn bundle_identity_is_stable_and_keeps_its_sidecar_outside_the_bundle() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let bundle = temporary.path().join("bundle");
        std::fs::create_dir(&bundle).expect("bundle directory");
        for name in [
            crate::model::PARAKEET_CONFIG,
            crate::model::PARAKEET_VOCAB,
            "nemo128.onnx",
            crate::model::PARAKEET_ENCODER,
            crate::model::PARAKEET_DECODER_JOINT,
        ] {
            std::fs::write(bundle.join(name), b"x").expect("fixture");
        }
        let artifact = ModelArtifact::detect(&bundle).expect("bundle artifact");

        let first = model_identity(&artifact).expect("identity");
        let second = model_identity(&artifact).expect("cached identity");

        assert_eq!(first.sha256.len(), 64);
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.size_bytes, 5);
        assert_eq!(first.engine, crate::domain::EngineKind::Parakeet);
        assert!(bundle_sidecar(&bundle).is_file());
        assert!(!bundle.join("config.json.sha256").exists());
    }

    #[test]
    fn canonical_bundle_hash_matches_the_identity() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let bundle = temporary.path().join("bundle");
        std::fs::create_dir(&bundle).expect("bundle directory");
        for name in [
            crate::model::PARAKEET_CONFIG,
            crate::model::PARAKEET_VOCAB,
            "nemo128.onnx",
            crate::model::PARAKEET_ENCODER,
            crate::model::PARAKEET_DECODER_JOINT,
        ] {
            std::fs::write(bundle.join(name), b"x").expect("fixture");
        }
        let artifact = ModelArtifact::detect(&bundle).expect("bundle artifact");
        let identity = model_identity(&artifact).expect("identity");

        let mut per_file = Vec::new();
        for path in artifact.files().expect("files") {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("file name")
                .to_owned();
            per_file.push((name, sha256_file(&path).expect("file hash")));
        }

        assert_eq!(identity.sha256, bundle_sha256(&per_file));
    }

    fn timed(start: u64, end: u64, text: &str) -> TimedText {
        TimedText {
            start: TimestampUs(start),
            end: TimestampUs(end),
            text: text.to_owned(),
        }
    }

    #[test]
    fn merge_groups_micro_segments_and_splits_on_punctuation_or_pause() {
        let merged = merge_timed_text(vec![
            timed(0, 900_000, "Ciao"),
            timed(1_000_000, 1_800_000, "come stai?"),
            timed(2_000_000, 2_900_000, "Bene"),
            timed(21_000_000, 21_900_000, "dopo"),
        ]);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "Ciao come stai?");
        assert_eq!(merged[0].start, TimestampUs(0));
        assert_eq!(merged[0].end, TimestampUs(1_800_000));
        assert_eq!(merged[1].text, "Bene");
        assert_eq!(merged[2].text, "dopo");
    }

    #[test]
    fn merge_caps_a_line_at_twenty_seconds() {
        let pieces = (0..5)
            .map(|index| timed(index * 5_000_000, (index + 1) * 5_000_000, "parola"))
            .collect();

        let merged = merge_timed_text(pieces);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].start, TimestampUs(0));
        assert_eq!(merged[0].end, TimestampUs(20_000_000));
        assert_eq!(merged[0].text, "parola parola parola parola");
        assert_eq!(merged[1].start, TimestampUs(20_000_000));
    }

    #[test]
    fn default_batch_wraps_the_live_transcription() {
        struct FixedAsr;

        impl AsrEngine for FixedAsr {
            fn transcribe(
                &mut self,
                _segment: &SpeechSegment,
                _language: &str,
                _config: &InferenceConfig,
            ) -> Result<AsrOutput> {
                Ok(AsrOutput {
                    text: "  ciao  ".to_owned(),
                    inference_duration_us: 1,
                })
            }
        }

        let pieces = FixedAsr
            .transcribe_batch(
                &vec![0.0; 16_000],
                "it",
                &InferenceConfig::default(),
                &Progress::new(|_| {}),
            )
            .expect("default batch should work");

        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].text, "ciao");
        assert_eq!(pieces[0].start, TimestampUs(0));
        assert_eq!(pieces[0].end, TimestampUs(1_000_000));
    }

    #[test]
    fn transcribing_sink_skips_empty_text() {
        struct SilentAsr;

        impl AsrEngine for SilentAsr {
            fn transcribe(
                &mut self,
                _segment: &SpeechSegment,
                _language: &str,
                _config: &InferenceConfig,
            ) -> Result<AsrOutput> {
                Ok(AsrOutput {
                    text: "   ".to_owned(),
                    inference_duration_us: 1,
                })
            }
        }

        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let path = temporary.path().join("transcript.jsonl");
        let identity = ModelIdentity {
            path: PathBuf::from("model.bin"),
            name: "model.bin".to_owned(),
            sha256: "sha".to_owned(),
            size_bytes: 1,
            engine: crate::domain::EngineKind::Whisper,
        };
        let mut sink = TranscribingSink::new(
            SilentAsr,
            identity,
            "it".to_owned(),
            InferenceConfig::default(),
        );

        sink.start(Uuid::nil(), &path).expect("start");
        sink.accept(&SpeechSegment {
            start: TimestampUs(0),
            end: TimestampUs(1_000_000),
            samples: vec![0.0; 16_000],
        })
        .expect("accept");
        sink.finish().expect("finish");

        let contents = std::fs::read_to_string(&path).expect("transcript should exist");
        assert!(contents.trim().is_empty());
    }
}
