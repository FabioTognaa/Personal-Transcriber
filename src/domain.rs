use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;
pub const TARGET_CHANNELS: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimestampUs(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSourceKind {
    Microphone,
    System,
    Mixed,
    File,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PcmChunk {
    pub source: AudioSourceKind,
    pub start: TimestampUs,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl PcmChunk {
    #[must_use]
    pub fn duration_us(&self) -> u64 {
        if self.sample_rate_hz == 0 || self.channels == 0 {
            return 0;
        }

        let frames = self.samples.len() as u64 / u64::from(self.channels);
        frames.saturating_mul(1_000_000) / u64::from(self.sample_rate_hz)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Running,
    TranscriptionPaused,
    Stopping,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub language: String,
    pub model_path: Option<PathBuf>,
    pub file_source: Option<PathBuf>,
    pub microphone_device: Option<String>,
    pub system_device: Option<String>,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

impl SessionConfig {
    #[must_use]
    pub fn italian(model_path: Option<PathBuf>) -> Self {
        Self {
            language: "it".to_owned(),
            model_path,
            file_source: None,
            microphone_device: None,
            system_device: None,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub state: SessionState,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub config: SessionConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub schema_version: u32,
    pub segment_id: Uuid,
    pub session_id: Uuid,
    pub start: TimestampUs,
    pub end: TimestampUs,
    pub text: String,
    pub language: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionStarted {
        schema_version: u32,
        session_id: Uuid,
        at: TimestampUs,
    },
    TranscriptionPaused {
        schema_version: u32,
        session_id: Uuid,
        at: TimestampUs,
    },
    TranscriptionResumed {
        schema_version: u32,
        session_id: Uuid,
        at: TimestampUs,
    },
    SessionStopped {
        schema_version: u32,
        session_id: Uuid,
        at: TimestampUs,
    },
    Error {
        schema_version: u32,
        session_id: Uuid,
        at: TimestampUs,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub schema_version: u32,
    pub generation: u64,
    pub action: ControlAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatus {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub state: SessionState,
    pub audio_position: TimestampUs,
    pub chunks_written: u64,
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub max_queue_depth: usize,
    pub applied_control_generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SCHEMA_VERSION;

    #[test]
    fn pcm_duration_uses_frames_and_sample_rate() {
        let chunk = PcmChunk {
            source: AudioSourceKind::Mixed,
            start: TimestampUs(0),
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 1_600],
        };

        assert_eq!(chunk.duration_us(), 100_000);
    }

    #[test]
    fn invalid_pcm_format_has_zero_duration() {
        let chunk = PcmChunk {
            source: AudioSourceKind::File,
            start: TimestampUs(0),
            sample_rate_hz: 0,
            channels: 1,
            samples: vec![0.0; 10],
        };

        assert_eq!(chunk.duration_us(), 0);
    }

    #[test]
    fn session_event_has_a_stable_tagged_json_shape() {
        let session_id = Uuid::nil();
        let event = SessionEvent::TranscriptionPaused {
            schema_version: SCHEMA_VERSION,
            session_id,
            at: TimestampUs(2_500_000),
        };

        let value = serde_json::to_value(event).expect("event should serialize");

        assert_eq!(value["type"], "transcription_paused");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["session_id"], session_id.to_string());
        assert_eq!(value["at"], 2_500_000);
    }

    #[test]
    fn transcript_segment_round_trips_without_changing_asr_text() {
        let segment = TranscriptSegment {
            schema_version: SCHEMA_VERSION,
            segment_id: Uuid::nil(),
            session_id: Uuid::nil(),
            start: TimestampUs(1_000_000),
            end: TimestampUs(2_250_000),
            text: "Testo ASR  non corretto.".to_owned(),
            language: "it".to_owned(),
            model: "fixture-model".to_owned(),
        };

        let encoded = serde_json::to_string(&segment).expect("segment should serialize");
        let decoded: TranscriptSegment =
            serde_json::from_str(&encoded).expect("segment should deserialize");

        assert_eq!(decoded, segment);
        assert_eq!(decoded.text, "Testo ASR  non corretto.");
    }
}
