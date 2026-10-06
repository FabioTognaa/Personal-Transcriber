use std::path::PathBuf;

use crate::domain::{ControlAction, SessionState};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid session path: {}", .0.display())]
    InvalidSessionPath(PathBuf),

    #[error("invalid {label} path {}: {reason}", path.display())]
    InvalidPath {
        label: &'static str,
        path: PathBuf,
        reason: String,
    },

    #[error(
        "insufficient disk space at {}: {available_bytes} bytes available, {required_bytes} required",
        path.display()
    )]
    InsufficientDiskSpace {
        path: PathBuf,
        required_bytes: u64,
        available_bytes: u64,
    },

    #[error("unsupported language `{0}`; supported languages are `it` and `en`")]
    UnsupportedLanguage(String),

    #[error("an active session already exists at {}", .0.display())]
    ActiveSessionExists(PathBuf),

    #[error("no current session exists under {}", .0.display())]
    NoCurrentSession(PathBuf),

    #[error("session has already finalized its audio: {}", .0.display())]
    SessionFinalized(PathBuf),

    #[error("unsupported WAV input: {0}")]
    UnsupportedWav(String),

    #[error("invalid segmenter configuration: {0}")]
    InvalidSegmenterConfig(String),

    #[error("a local whisper model is required; pass --model <GGML .bin>")]
    ModelRequired,

    #[error("invalid inference configuration: {0}")]
    InvalidInferenceConfig(String),

    #[error("invalid transcript {} at line {line}: {reason}", path.display())]
    InvalidTranscript {
        path: PathBuf,
        line: usize,
        reason: String,
    },

    #[error("invalid export: {0}")]
    InvalidExport(String),

    #[error("local ASR failed: {0}")]
    Asr(String),

    #[error("local audio subsystem failed: {0}")]
    Audio(String),

    #[error("one or more required doctor checks failed")]
    DoctorFailed,

    #[error("timed out waiting for the session to apply a control request")]
    ControlTimeout,

    #[error("operation interrupted before the session started")]
    OperationInterrupted,

    #[error("session interrupted; partial data was finalized at {}", .0.display())]
    SessionInterrupted(PathBuf),

    #[error("cannot apply {action:?} while the session is {state:?}")]
    InvalidControlState {
        action: ControlAction,
        state: SessionState,
    },

    #[error("audio worker terminated unexpectedly")]
    AudioWorkerPanicked,

    #[error("segmentation worker terminated unexpectedly")]
    SegmentationWorkerPanicked,

    #[error("ASR worker terminated unexpectedly")]
    AsrWorkerPanicked,

    #[error("the system clock is before the Unix epoch")]
    SystemClockBeforeUnixEpoch,

    #[error("timestamp exceeds the supported range")]
    TimestampOverflow,

    #[error("WAV processing failed: {0}")]
    Wav(#[from] hound::Error),

    #[error("failed to serialize local session data: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
