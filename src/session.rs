use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, bounded};
use uuid::Uuid;

use crate::audio::FileAudioSource;
use crate::domain::{
    ControlAction, ControlRequest, PcmChunk, SessionConfig, SessionEvent, SessionMetadata,
    SessionState, SessionStatus, TimestampUs,
};
use crate::schema::{CURRENT_SESSION_FILE, SCHEMA_VERSION};
use crate::storage::{CurrentSession, SessionStorage, read_json, unix_time_ms};
use crate::{Error, Result};

const QUEUE_CAPACITY: usize = 64;
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
}

pub trait TranscriptionSink {
    fn accept(&mut self, chunk: &PcmChunk) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct NullTranscriptionSink;

impl TranscriptionSink for NullTranscriptionSink {
    fn accept(&mut self, _chunk: &PcmChunk) -> Result<()> {
        Ok(())
    }
}

pub fn run(options: StartOptions) -> Result<PathBuf> {
    run_with_sink(options, &mut NullTranscriptionSink)
}

pub fn run_with_sink(options: StartOptions, sink: &mut impl TranscriptionSink) -> Result<PathBuf> {
    ensure_no_active_session(&options.sessions_dir)?;
    let _runner_guard = RunnerGuard::acquire(&options.sessions_dir)?;
    let source = FileAudioSource::open(&options.input_wav)?;

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
    let max_queue_depth = Arc::new(AtomicUsize::new(0));
    let producer_max = Arc::clone(&max_queue_depth);
    let producer = thread::spawn(move || {
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
                // Keep the source paced through the final chunk as well.
                let final_due = due + Duration::from_micros(duration);
                if let Some(delay) = final_due.checked_duration_since(Instant::now()) {
                    thread::sleep(delay);
                }
            }
        }
    });

    loop {
        apply_control_if_present(&mut storage, &mut metadata, &mut status)?;
        if status.state == SessionState::Stopping {
            break;
        }

        match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(chunk) => {
                storage.write_audio(&chunk)?;
                if status.state == SessionState::Running {
                    sink.accept(&chunk)?;
                }
                status.audio_position =
                    TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
                status.chunks_written += 1;
                status.queue_depth = receiver.len();
                status.max_queue_depth = max_queue_depth.load(Ordering::Relaxed);
                storage.write_status(&status)?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    while let Ok(chunk) = receiver.try_recv() {
        storage.write_audio(&chunk)?;
        status.audio_position = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
        status.chunks_written += 1;
    }
    drop(receiver);
    producer.join().map_err(|_| Error::AudioWorkerPanicked)?;
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
