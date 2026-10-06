use std::path::PathBuf;

use crate::domain::{ControlAction, SessionState};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("command `{0}` is not implemented yet")]
    CommandNotImplemented(&'static str),

    #[error("invalid session path: {}", .0.display())]
    InvalidSessionPath(PathBuf),

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

    #[error("local ASR failed: {0}")]
    Asr(String),

    #[error("timed out waiting for the session to apply a control request")]
    ControlTimeout,

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
