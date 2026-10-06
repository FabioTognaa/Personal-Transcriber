use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 3;
pub const SESSION_METADATA_FILE: &str = "session.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const TRANSCRIPT_FILE: &str = "transcript.jsonl";
pub const STATUS_FILE: &str = "status.json";
pub const CONTROL_FILE: &str = "control.json";
pub const CURRENT_SESSION_FILE: &str = "current-session.json";
pub const AUDIO_DIRECTORY: &str = "audio";
pub const MIXED_AUDIO_FILE: &str = "mixed.wav";
pub const MICROPHONE_AUDIO_FILE: &str = "microphone.wav";
pub const SYSTEM_AUDIO_FILE: &str = "system.wav";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    pub root: PathBuf,
    pub metadata: PathBuf,
    pub events: PathBuf,
    pub transcript: PathBuf,
    pub status: PathBuf,
    pub control: PathBuf,
    pub mixed_audio: PathBuf,
    pub microphone_audio: PathBuf,
    pub system_audio: PathBuf,
    pub audio: PathBuf,
}

impl SessionPaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();

        Self {
            metadata: root.join(SESSION_METADATA_FILE),
            events: root.join(EVENTS_FILE),
            transcript: root.join(TRANSCRIPT_FILE),
            status: root.join(STATUS_FILE),
            control: root.join(CONTROL_FILE),
            mixed_audio: root.join(AUDIO_DIRECTORY).join(MIXED_AUDIO_FILE),
            microphone_audio: root.join(AUDIO_DIRECTORY).join(MICROPHONE_AUDIO_FILE),
            system_audio: root.join(AUDIO_DIRECTORY).join(SYSTEM_AUDIO_FILE),
            audio: root.join(AUDIO_DIRECTORY),
            root,
        }
    }

    #[must_use]
    pub fn is_within(&self, parent: &Path) -> bool {
        self.root.starts_with(parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_paths_follow_the_canonical_layout() {
        let paths = SessionPaths::new("/tmp/personal-transcriber/session-1");

        assert_eq!(
            paths.metadata,
            PathBuf::from("/tmp/personal-transcriber/session-1/session.json")
        );
        assert_eq!(
            paths.transcript,
            PathBuf::from("/tmp/personal-transcriber/session-1/transcript.jsonl")
        );
        assert_eq!(
            paths.audio,
            PathBuf::from("/tmp/personal-transcriber/session-1/audio")
        );
    }
}
