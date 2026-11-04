pub mod asr;
pub mod audio;
pub mod cli;
pub mod control;
pub mod diagnostics;
pub mod domain;
pub mod error;
pub mod export;
pub mod live_audio;
pub mod logging;
pub mod model;
#[cfg(feature = "parakeet")]
pub mod parakeet;
pub mod schema;
pub mod segment;
pub mod session;
pub mod storage;
pub mod system_audio;
pub mod transcribe;
pub mod validation;

pub use error::{Error, Result};
