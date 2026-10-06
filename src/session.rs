use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, TrySendError, bounded};
use uuid::Uuid;

use crate::asr::{TranscribingSink, WhisperEngine, model_identity, validate_inference_config};
use crate::audio::FileAudioSource;
use crate::domain::{
    AsrMetrics, ControlAction, ControlRequest, InferenceConfig, ModelIdentity, PcmChunk,
    SegmentationMetrics, SegmenterConfig, SessionConfig, SessionEvent, SessionMetadata,
    SessionState, SessionStatus, SpeechSegment, TimestampUs,
};
use crate::schema::{CURRENT_SESSION_FILE, SCHEMA_VERSION};
use crate::segment::Segmenter;
use crate::storage::{CurrentSession, SessionStorage, read_json, unix_time_ms};
use crate::{Error, Result};

const QUEUE_CAPACITY: usize = 64;
const SEGMENTATION_QUEUE_CAPACITY: usize = 64;
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(5);
const RUNNER_LOCK_FILE: &str = ".runner.lock";

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub sessions_dir: PathBuf,
    pub input_wav: PathBuf,
    pub model: Option<PathBuf>,
    pub language: String,
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
    pub segmenter: SegmenterConfig,
    pub inference: InferenceConfig,
    pub model_identity: Option<ModelIdentity>,
}

pub trait SegmentSink {
    fn start(&mut self, _session_id: Uuid, _transcript_path: &Path) -> Result<()> {
        Ok(())
    }

    fn accept(&mut self, segment: &SpeechSegment) -> Result<()>;

    fn finish(&mut self) -> Result<()> {
        Ok(())
    }

    fn status_handle(&self) -> Arc<Mutex<SegmentSinkStatus>> {
        Arc::new(Mutex::new(SegmentSinkStatus::default()))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SegmentSinkStatus {
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub max_queue_depth: usize,
    pub metrics: AsrMetrics,
}

#[derive(Debug, Default)]
pub struct NullSegmentSink;

impl SegmentSink for NullSegmentSink {
    fn accept(&mut self, _segment: &SpeechSegment) -> Result<()> {
        Ok(())
    }
}

pub fn run(options: StartOptions) -> Result<PathBuf> {
    let shutdown = ShutdownSignals::install()?;
    validate_language(&options.language)?;
    validate_inference_config(&options.inference)?;
    let model_path = options.model.as_deref().ok_or(Error::ModelRequired)?;
    crate::validation::validate_existing_file(model_path, "model")?;
    let identity = model_identity(model_path)?;
    let engine = WhisperEngine::load(model_path, &options.inference)?;
    let mut sink = TranscribingSink::new(
        engine,
        identity.clone(),
        options.language.clone(),
        options.inference.clone(),
    );
    let mut options = options;
    options.model_identity = Some(identity);
    if shutdown.requested.load(Ordering::Relaxed) {
        return Err(Error::OperationInterrupted);
    }
    run_with_sink_internal(options, &mut sink, Some(&shutdown.requested))
}

pub fn run_with_sink(
    options: StartOptions,
    sink: &mut (impl SegmentSink + Send),
) -> Result<PathBuf> {
    run_with_sink_internal(options, sink, None)
}

pub fn run_with_sink_until(
    options: StartOptions,
    sink: &mut (impl SegmentSink + Send),
    shutdown_requested: &AtomicBool,
) -> Result<PathBuf> {
    run_with_sink_internal(options, sink, Some(shutdown_requested))
}

fn run_with_sink_internal(
    options: StartOptions,
    sink: &mut (impl SegmentSink + Send),
    shutdown_requested: Option<&AtomicBool>,
) -> Result<PathBuf> {
    crate::validation::validate_directory_target(&options.sessions_dir, "sessions directory")?;
    ensure_no_active_session(&options.sessions_dir)?;
    let _runner_guard = RunnerGuard::acquire(&options.sessions_dir)?;
    validate_language(&options.language)?;
    validate_inference_config(&options.inference)?;
    crate::validation::validate_existing_file(&options.input_wav, "input WAV")?;
    let source = FileAudioSource::open(&options.input_wav)?;
    let segmenter_config = options.segmenter.clone();
    let segmenter = Segmenter::new(segmenter_config.clone())?;
    crate::validation::ensure_disk_space(
        &options.sessions_dir,
        crate::validation::estimated_session_bytes(source.sample_count()),
    )?;

    let session_id = Uuid::new_v4();
    let started_at_unix_ms = unix_time_ms()?;
    let mut metadata = SessionMetadata {
        schema_version: SCHEMA_VERSION,
        session_id,
        state: SessionState::Starting,
        started_at_unix_ms,
        ended_at_unix_ms: None,
        config: SessionConfig {
            language: options.language,
            model_path: options.model,
            model: options.model_identity,
            file_source: Some(options.input_wav),
            microphone_device: options.microphone,
            system_device: options.system_audio,
            sample_rate_hz: crate::domain::TARGET_SAMPLE_RATE_HZ,
            channels: crate::domain::TARGET_CHANNELS,
            segmenter: options.segmenter,
            inference: options.inference,
        },
    };
    let mut storage = SessionStorage::create(&options.sessions_dir, &metadata)?;
    sink.start(session_id, &storage.paths.transcript)?;
    let sink_status_handle = sink.status_handle();
    let mut status = SessionStatus {
        schema_version: SCHEMA_VERSION,
        session_id,
        state: SessionState::Starting,
        audio_position: TimestampUs(0),
        chunks_written: 0,
        queue_depth: 0,
        queue_capacity: QUEUE_CAPACITY,
        max_queue_depth: 0,
        segmentation_queue_depth: 0,
        segmentation_queue_capacity: SEGMENTATION_QUEUE_CAPACITY,
        max_segmentation_queue_depth: 0,
        segmentation_replay_required: false,
        segmentation: SegmentationMetrics::default(),
        asr_queue_depth: 0,
        asr_queue_capacity: 0,
        max_asr_queue_depth: 0,
        asr_replay_required: false,
        asr: AsrMetrics::default(),
        applied_control_generation: 0,
    };

    storage.append_event(&SessionEvent::SessionStarted {
        schema_version: SCHEMA_VERSION,
        session_id,
        at: TimestampUs(0),
    })?;
    metadata.state = SessionState::Running;
    status.state = SessionState::Running;
    storage.write_metadata(&metadata)?;
    storage.write_status(&status)?;

    let (sender, receiver) = bounded(QUEUE_CAPACITY);
    let (segmentation_sender, segmentation_receiver) = bounded(SEGMENTATION_QUEUE_CAPACITY);
    let max_queue_depth = Arc::new(AtomicUsize::new(0));
    let max_segmentation_queue_depth = Arc::new(AtomicUsize::new(0));
    let segmentation_metrics = Arc::new(Mutex::new(SegmentationMetrics::default()));
    let mut interrupted = false;
    let processing_result = thread::scope(|scope| -> Result<()> {
        let producer_max = Arc::clone(&max_queue_depth);
        let producer = scope.spawn(move || {
            let stream_start = Instant::now();
            for chunk in source.chunks() {
                let due = stream_start + Duration::from_micros(chunk.start.0);
                if let Some(delay) = due.checked_duration_since(Instant::now()) {
                    thread::sleep(delay);
                }
                let duration = chunk.duration_us();
                if sender.send(chunk).is_err() {
                    return;
                }
                producer_max.fetch_max(sender.len(), Ordering::Relaxed);
                if sender.is_empty() && duration > 0 {
                    let final_due = due + Duration::from_micros(duration);
                    if let Some(delay) = final_due.checked_duration_since(Instant::now()) {
                        thread::sleep(delay);
                    }
                }
            }
        });

        let worker_metrics = Arc::clone(&segmentation_metrics);
        let segmentation_worker = scope.spawn(move || {
            let result = run_segmentation_worker(
                segmenter,
                segmentation_receiver,
                &mut *sink,
                &worker_metrics,
            );
            (result, sink)
        });

        let mut segment_drain = true;
        let mut segmentation_replay_required = false;
        let mut eligible_ranges = Vec::new();
        loop {
            let previous_state = status.state;
            apply_control_if_present(&mut storage, &mut metadata, &mut status)?;
            interrupted |= apply_shutdown_if_requested(
                shutdown_requested,
                &storage,
                &mut metadata,
                &mut status,
            )?;
            if previous_state == SessionState::Running
                && status.state == SessionState::TranscriptionPaused
            {
                segmentation_sender
                    .send(SegmentationCommand::Flush)
                    .map_err(|_| Error::SegmentationWorkerPanicked)?;
            }
            if status.state == SessionState::Stopping {
                segment_drain = previous_state == SessionState::Running;
                break;
            }

            match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
                Ok(chunk) => {
                    storage.write_audio(&chunk)?;
                    if status.state == SessionState::Running {
                        record_eligible_range(&mut eligible_ranges, &chunk);
                        if !segmentation_replay_required
                            && !enqueue_for_segmentation(
                                &segmentation_sender,
                                chunk.clone(),
                                &max_segmentation_queue_depth,
                            )?
                        {
                            segmentation_replay_required = true;
                            status.segmentation_replay_required = true;
                            status.asr_replay_required = true;
                        }
                    }
                    update_status_for_chunk(&mut status, &chunk);
                    refresh_queue_status(
                        &mut status,
                        receiver.len(),
                        &max_queue_depth,
                        segmentation_sender.len(),
                        &max_segmentation_queue_depth,
                        &segmentation_metrics,
                        &sink_status_handle,
                    );
                    storage.write_status(&status)?;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        while let Ok(chunk) = receiver.try_recv() {
            storage.write_audio(&chunk)?;
            if segment_drain {
                record_eligible_range(&mut eligible_ranges, &chunk);
                if !segmentation_replay_required
                    && !enqueue_for_segmentation(
                        &segmentation_sender,
                        chunk.clone(),
                        &max_segmentation_queue_depth,
                    )?
                {
                    segmentation_replay_required = true;
                    status.segmentation_replay_required = true;
                    status.asr_replay_required = true;
                }
            }
            update_status_for_chunk(&mut status, &chunk);
        }
        drop(receiver);

        segmentation_sender
            .send(SegmentationCommand::Flush)
            .map_err(|_| Error::SegmentationWorkerPanicked)?;
        segmentation_sender
            .send(if segmentation_replay_required {
                SegmentationCommand::ShutdownDiscard
            } else {
                SegmentationCommand::ShutdownAndEmit
            })
            .map_err(|_| Error::SegmentationWorkerPanicked)?;
        drop(segmentation_sender);

        producer.join().map_err(|_| Error::AudioWorkerPanicked)?;
        let (worker_result, sink) = segmentation_worker
            .join()
            .map_err(|_| Error::SegmentationWorkerPanicked)?;
        let sink_processing_result: Result<()> = (|| {
            worker_result?;
            if segmentation_replay_required {
                storage.finalize_audio()?;
                let replay_metrics = replay_segmentation(
                    &storage.paths.mixed_audio,
                    &eligible_ranges,
                    segmenter_config,
                    sink,
                )?;
                *segmentation_metrics
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = replay_metrics;
            }
            Ok(())
        })();
        let finish_result = sink.finish();
        finish_result?;
        sink_processing_result?;
        refresh_queue_status(
            &mut status,
            0,
            &max_queue_depth,
            0,
            &max_segmentation_queue_depth,
            &segmentation_metrics,
            &sink_status_handle,
        );
        refresh_sink_status(&mut status, &sink_status_handle);
        Ok(())
    });

    if let Err(error) = processing_result {
        metadata.state = SessionState::Failed;
        metadata.ended_at_unix_ms = Some(unix_time_ms()?);
        status.state = SessionState::Failed;
        storage.append_event(&SessionEvent::Error {
            schema_version: SCHEMA_VERSION,
            session_id,
            at: status.audio_position,
            message: error.to_string(),
        })?;
        storage.finalize_audio()?;
        storage.write_metadata(&metadata)?;
        storage.write_status(&status)?;
        return Err(error);
    }

    if interrupted {
        metadata.state = SessionState::Failed;
        metadata.ended_at_unix_ms = Some(unix_time_ms()?);
        status.state = SessionState::Failed;
        storage.append_event(&SessionEvent::Error {
            schema_version: SCHEMA_VERSION,
            session_id,
            at: status.audio_position,
            message: "termination signal received; partial session finalized".to_owned(),
        })?;
        storage.finalize_audio()?;
        storage.write_metadata(&metadata)?;
        storage.write_status(&status)?;
        return Err(Error::SessionInterrupted(storage.paths.root.clone()));
    }

    storage.append_event(&SessionEvent::SessionStopped {
        schema_version: SCHEMA_VERSION,
        session_id,
        at: status.audio_position,
    })?;
    storage.finalize_audio()?;
    metadata.state = SessionState::Completed;
    metadata.ended_at_unix_ms = Some(unix_time_ms()?);
    status.state = SessionState::Completed;
    status.queue_depth = 0;
    storage.write_metadata(&metadata)?;
    storage.write_status(&status)?;

    Ok(storage.paths.root)
}

fn validate_language(language: &str) -> Result<()> {
    if language == "it" {
        Ok(())
    } else {
        Err(Error::UnsupportedLanguage(language.to_owned()))
    }
}

fn apply_shutdown_if_requested(
    shutdown_requested: Option<&AtomicBool>,
    storage: &SessionStorage,
    metadata: &mut SessionMetadata,
    status: &mut SessionStatus,
) -> Result<bool> {
    let requested = shutdown_requested.is_some_and(|requested| requested.load(Ordering::Relaxed));
    if requested
        && matches!(
            status.state,
            SessionState::Running | SessionState::TranscriptionPaused
        )
    {
        status.state = SessionState::Stopping;
        metadata.state = SessionState::Stopping;
        storage.write_metadata(metadata)?;
        storage.write_status(status)?;
        return Ok(true);
    }
    Ok(false)
}

struct ShutdownSignals {
    requested: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl ShutdownSignals {
    fn install() -> Result<Self> {
        use signal_hook::consts::signal::{SIGINT, SIGTERM};

        let requested = Arc::new(AtomicBool::new(false));
        let mut registrations = Vec::new();
        for signal in [SIGINT, SIGTERM] {
            match signal_hook::flag::register(signal, Arc::clone(&requested)) {
                Ok(registration) => registrations.push(registration),
                Err(error) => {
                    for registration in registrations {
                        signal_hook::low_level::unregister(registration);
                    }
                    return Err(error.into());
                }
            }
        }
        Ok(Self {
            requested,
            registrations,
        })
    }
}

impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

enum SegmentationCommand {
    Chunk(PcmChunk),
    Flush,
    ShutdownAndEmit,
    ShutdownDiscard,
}

fn run_segmentation_worker(
    mut segmenter: Segmenter,
    receiver: crossbeam_channel::Receiver<SegmentationCommand>,
    sink: &mut impl SegmentSink,
    shared_metrics: &Mutex<SegmentationMetrics>,
) -> Result<()> {
    while let Ok(command) = receiver.recv() {
        let (segments, shutdown, emit) = match command {
            SegmentationCommand::Chunk(chunk) => (segmenter.push_chunk(chunk), false, true),
            SegmentationCommand::Flush => (segmenter.flush(), false, true),
            SegmentationCommand::ShutdownAndEmit => (segmenter.flush(), true, true),
            SegmentationCommand::ShutdownDiscard => (segmenter.flush(), true, false),
        };
        if emit {
            for segment in &segments {
                sink.accept(segment)?;
            }
        }
        *shared_metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = segmenter.metrics();
        if shutdown {
            return Ok(());
        }
    }

    for segment in &segmenter.flush() {
        sink.accept(segment)?;
    }
    *shared_metrics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = segmenter.metrics();
    Ok(())
}

fn enqueue_for_segmentation(
    sender: &crossbeam_channel::Sender<SegmentationCommand>,
    chunk: PcmChunk,
    max_queue_depth: &AtomicUsize,
) -> Result<bool> {
    match sender.try_send(SegmentationCommand::Chunk(chunk)) {
        Ok(()) => {
            max_queue_depth.fetch_max(sender.len(), Ordering::Relaxed);
            Ok(true)
        }
        Err(TrySendError::Full(_)) => Ok(false),
        Err(TrySendError::Disconnected(_)) => Err(Error::SegmentationWorkerPanicked),
    }
}

fn record_eligible_range(ranges: &mut Vec<(TimestampUs, TimestampUs)>, chunk: &PcmChunk) {
    let end = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
    if let Some((_, previous_end)) = ranges.last_mut()
        && *previous_end == chunk.start
    {
        *previous_end = end;
        return;
    }
    ranges.push((chunk.start, end));
}

fn replay_segmentation(
    audio_path: &Path,
    eligible_ranges: &[(TimestampUs, TimestampUs)],
    config: SegmenterConfig,
    sink: &mut impl SegmentSink,
) -> Result<SegmentationMetrics> {
    let source = FileAudioSource::open(audio_path)?;
    let mut segmenter = Segmenter::new(config)?;
    let mut range_index = 0;
    let mut was_eligible = false;

    for chunk in source.chunks() {
        let chunk_end = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
        while range_index < eligible_ranges.len() && eligible_ranges[range_index].1 <= chunk.start {
            range_index += 1;
        }
        let eligible = eligible_ranges
            .get(range_index)
            .is_some_and(|(start, end)| *start <= chunk.start && chunk_end <= *end);
        if was_eligible && !eligible {
            for segment in segmenter.flush() {
                sink.accept(&segment)?;
            }
        }
        if eligible {
            for segment in segmenter.push_chunk(chunk) {
                sink.accept(&segment)?;
            }
        }
        was_eligible = eligible;
    }
    for segment in segmenter.flush() {
        sink.accept(&segment)?;
    }
    Ok(segmenter.metrics())
}

fn update_status_for_chunk(status: &mut SessionStatus, chunk: &PcmChunk) {
    status.audio_position = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
    status.chunks_written = status.chunks_written.saturating_add(1);
}

#[allow(clippy::too_many_arguments)]
fn refresh_queue_status(
    status: &mut SessionStatus,
    queue_depth: usize,
    max_queue_depth: &AtomicUsize,
    segmentation_queue_depth: usize,
    max_segmentation_queue_depth: &AtomicUsize,
    segmentation_metrics: &Mutex<SegmentationMetrics>,
    sink_status: &Mutex<SegmentSinkStatus>,
) {
    status.queue_depth = queue_depth;
    status.max_queue_depth = max_queue_depth.load(Ordering::Relaxed);
    status.segmentation_queue_depth = segmentation_queue_depth;
    status.max_segmentation_queue_depth = max_segmentation_queue_depth.load(Ordering::Relaxed);
    status.segmentation = *segmentation_metrics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    refresh_sink_status(status, sink_status);
}

fn refresh_sink_status(status: &mut SessionStatus, sink_status: &Mutex<SegmentSinkStatus>) {
    let sink_status = *sink_status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    status.asr_queue_depth = sink_status.queue_depth;
    status.asr_queue_capacity = sink_status.queue_capacity;
    status.max_asr_queue_depth = sink_status.max_queue_depth;
    status.asr = sink_status.metrics;
}

fn apply_control_if_present(
    storage: &mut SessionStorage,
    metadata: &mut SessionMetadata,
    status: &mut SessionStatus,
) -> Result<()> {
    if !storage.paths.control.exists() {
        return Ok(());
    }
    let request: ControlRequest = read_json(&storage.paths.control)?;
    if request.generation <= status.applied_control_generation {
        return Ok(());
    }

    match (request.action, status.state) {
        (ControlAction::Pause, SessionState::Running) => {
            status.state = SessionState::TranscriptionPaused;
            metadata.state = status.state;
            storage.append_event(&SessionEvent::TranscriptionPaused {
                schema_version: SCHEMA_VERSION,
                session_id: status.session_id,
                at: status.audio_position,
            })?;
        }
        (ControlAction::Resume, SessionState::TranscriptionPaused) => {
            status.state = SessionState::Running;
            metadata.state = status.state;
            storage.append_event(&SessionEvent::TranscriptionResumed {
                schema_version: SCHEMA_VERSION,
                session_id: status.session_id,
                at: status.audio_position,
            })?;
        }
        (ControlAction::Stop, SessionState::Running | SessionState::TranscriptionPaused) => {
            status.state = SessionState::Stopping;
            metadata.state = status.state;
        }
        _ => {}
    }

    status.applied_control_generation = request.generation;
    storage.write_metadata(metadata)?;
    storage.write_status(status)
}

fn ensure_no_active_session(sessions_dir: &Path) -> Result<()> {
    let current_path = sessions_dir.join(CURRENT_SESSION_FILE);
    if !current_path.exists() {
        return Ok(());
    }
    let current: CurrentSession = read_json(&current_path)?;
    let status_path = crate::schema::SessionPaths::new(&current.root).status;
    if !status_path.exists() {
        return Ok(());
    }
    let status: SessionStatus = read_json(&status_path)?;
    if matches!(
        status.state,
        SessionState::Starting
            | SessionState::Running
            | SessionState::TranscriptionPaused
            | SessionState::Stopping
    ) {
        return Err(Error::ActiveSessionExists(current.root));
    }
    Ok(())
}

struct RunnerGuard {
    path: PathBuf,
}

impl RunnerGuard {
    fn acquire(sessions_dir: &Path) -> Result<Self> {
        fs::create_dir_all(sessions_dir)?;
        let path = sessions_dir.join(RUNNER_LOCK_FILE);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::ActiveSessionExists(sessions_dir.to_path_buf())
                } else {
                    Error::Io(error)
                }
            })?;
        Ok(Self { path })
    }
}

impl Drop for RunnerGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
