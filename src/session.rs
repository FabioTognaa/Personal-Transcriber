use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, TrySendError, bounded};
use uuid::Uuid;

use crate::audio::FileAudioSource;
use crate::domain::{
    ControlAction, ControlRequest, PcmChunk, SegmentationMetrics, SegmenterConfig, SessionConfig,
    SessionEvent, SessionMetadata, SessionState, SessionStatus, SpeechSegment, TimestampUs,
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
}

pub trait SegmentSink {
    fn accept(&mut self, segment: &SpeechSegment) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct NullSegmentSink;

impl SegmentSink for NullSegmentSink {
    fn accept(&mut self, _segment: &SpeechSegment) -> Result<()> {
        Ok(())
    }
}

pub fn run(options: StartOptions) -> Result<PathBuf> {
    run_with_sink(options, &mut NullSegmentSink)
}

pub fn run_with_sink(
    options: StartOptions,
    sink: &mut (impl SegmentSink + Send),
) -> Result<PathBuf> {
    ensure_no_active_session(&options.sessions_dir)?;
    let _runner_guard = RunnerGuard::acquire(&options.sessions_dir)?;
    let source = FileAudioSource::open(&options.input_wav)?;
    let segmenter_config = options.segmenter.clone();

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
            file_source: Some(options.input_wav),
            microphone_device: options.microphone,
            system_device: options.system_audio,
            sample_rate_hz: crate::domain::TARGET_SAMPLE_RATE_HZ,
            channels: crate::domain::TARGET_CHANNELS,
            segmenter: options.segmenter,
        },
    };
    let mut storage = SessionStorage::create(&options.sessions_dir, &metadata)?;
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
    let segmenter = Segmenter::new(segmenter_config.clone())?;

    thread::scope(|scope| -> Result<()> {
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
        refresh_queue_status(
            &mut status,
            0,
            &max_queue_depth,
            0,
            &max_segmentation_queue_depth,
            &segmentation_metrics,
        );
        Ok(())
    })?;

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
    let mut completed_segments = Vec::new();
    while let Ok(command) = receiver.recv() {
        let shutdown = matches!(
            &command,
            SegmentationCommand::ShutdownAndEmit | SegmentationCommand::ShutdownDiscard
        );
        let emit = !matches!(&command, SegmentationCommand::ShutdownDiscard);
        let segments = match command {
            SegmentationCommand::Chunk(chunk) => segmenter.push_chunk(chunk),
            SegmentationCommand::Flush
            | SegmentationCommand::ShutdownAndEmit
            | SegmentationCommand::ShutdownDiscard => segmenter.flush(),
        };
        completed_segments.extend(segments);
        *shared_metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = segmenter.metrics();
        if shutdown {
            if emit {
                for segment in &completed_segments {
                    sink.accept(segment)?;
                }
            }
            return Ok(());
        }
    }

    completed_segments.extend(segmenter.flush());
    for segment in &completed_segments {
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
) {
    status.queue_depth = queue_depth;
    status.max_queue_depth = max_queue_depth.load(Ordering::Relaxed);
    status.segmentation_queue_depth = segmentation_queue_depth;
    status.max_segmentation_queue_depth = max_segmentation_queue_depth.load(Ordering::Relaxed);
    status.segmentation = *segmentation_metrics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
