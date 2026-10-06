use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::{Sender, bounded};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::domain::{
    InferenceConfig, InferenceStrategy, ModelIdentity, SpeechSegment, TranscriptSegment,
};
use crate::schema::SCHEMA_VERSION;
use crate::session::{SegmentSink, SegmentSinkStatus};
use crate::storage::TranscriptWriter;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrOutput {
    pub text: String,
    pub inference_duration_us: u64,
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
                        inference: inference.clone(),
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
        parameters.set_temperature(config.temperature);
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
}

pub fn model_identity(path: &Path) -> Result<ModelIdentity> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(Error::Asr(format!(
            "model is not a non-empty file: {}",
            path.display()
        )));
    }

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
    let sha256 = format!("{:x}", hasher.finalize());
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
    Ok(())
}
