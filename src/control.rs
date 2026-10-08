use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::domain::{
    ControlAction, ControlRequest, SessionMetadata, SessionState, SessionStatus, TimestampUs,
};
use crate::schema::{CURRENT_SESSION_FILE, SCHEMA_VERSION, SessionPaths};
use crate::storage::{CurrentSession, ExclusiveFileLock, atomic_write_json, read_json};
use crate::{Error, Result};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CONTROL_LOCK_FILE: &str = ".control.lock";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusReport {
    pub session_root: PathBuf,
    pub mixed_audio: PathBuf,
    pub transcript: PathBuf,
    pub events: PathBuf,
    pub asr_position_lag: Option<TimestampUs>,
    pub metadata: SessionMetadata,
    pub runtime: SessionStatus,
}

pub fn current_status(sessions_dir: &Path) -> Result<SessionStatus> {
    let (_, paths) = current_session(sessions_dir)?;
    read_json(&paths.status)
}

pub fn status_report(sessions_dir: &Path) -> Result<StatusReport> {
    let (_, paths) = current_session(sessions_dir)?;
    let metadata: SessionMetadata = read_json(&paths.metadata)?;
    let runtime: SessionStatus = read_json(&paths.status)?;
    let asr_position_lag = (runtime.state != SessionState::TranscriptionPaused).then(|| {
        TimestampUs(
            runtime
                .audio_position
                .0
                .saturating_sub(runtime.asr.last_segment_end.0),
        )
    });

    Ok(StatusReport {
        session_root: paths.root,
        mixed_audio: paths.mixed_audio,
        transcript: paths.transcript,
        events: paths.events,
        asr_position_lag,
        metadata,
        runtime,
    })
}

pub fn request(sessions_dir: &Path, action: ControlAction) -> Result<SessionStatus> {
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    let _control_lock = acquire_control_lock(sessions_dir, deadline)?;
    let (_, paths) = current_session(sessions_dir)?;
    let status: SessionStatus = read_json(&paths.status)?;

    match (action, status.state) {
        (ControlAction::Stop, SessionState::Completed)
        | (ControlAction::Pause, SessionState::TranscriptionPaused)
        | (ControlAction::Resume, SessionState::Running) => return Ok(status),
        (ControlAction::Stop, SessionState::Stopping) => return wait_for_completed(&paths),
        (ControlAction::Pause, SessionState::Running)
        | (ControlAction::Resume, SessionState::TranscriptionPaused)
        | (ControlAction::Stop, SessionState::Running | SessionState::TranscriptionPaused) => {}
        _ => {
            return Err(Error::InvalidControlState {
                action,
                state: status.state,
            });
        }
    }

    let pending_generation = if paths.control.exists() {
        let pending: ControlRequest = read_json(&paths.control)?;
        pending.generation
    } else {
        0
    };
    let generation = status
        .applied_control_generation
        .max(pending_generation)
        .saturating_add(1);
    atomic_write_json(
        &paths.control,
        &ControlRequest {
            schema_version: SCHEMA_VERSION,
            generation,
            action,
        },
    )?;

    while Instant::now() < deadline {
        let updated: SessionStatus = read_json(&paths.status)?;
        if updated.applied_control_generation >= generation {
            if action == ControlAction::Stop {
                return wait_for_completed(&paths);
            }
            if control_action_satisfied(action, updated.state) {
                return Ok(updated);
            }
            return Err(Error::InvalidControlState {
                action,
                state: updated.state,
            });
        }
        if updated.state == SessionState::Completed && action == ControlAction::Stop {
            return Ok(updated);
        }
        if matches!(
            updated.state,
            SessionState::Completed | SessionState::Failed
        ) {
            return Err(Error::InvalidControlState {
                action,
                state: updated.state,
            });
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }
    Err(Error::ControlTimeout)
}

fn acquire_control_lock(sessions_dir: &Path, deadline: Instant) -> Result<ExclusiveFileLock> {
    std::fs::create_dir_all(sessions_dir)?;
    let lock_path = sessions_dir.join(CONTROL_LOCK_FILE);
    while Instant::now() < deadline {
        if let Some(lock) = ExclusiveFileLock::try_acquire(&lock_path)? {
            return Ok(lock);
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }
    Err(Error::ControlTimeout)
}

fn wait_for_completed(paths: &SessionPaths) -> Result<SessionStatus> {
    wait_for_completed_until(paths, Instant::now() + STOP_COMPLETION_TIMEOUT)
}

fn wait_for_completed_until(paths: &SessionPaths, deadline: Instant) -> Result<SessionStatus> {
    while Instant::now() < deadline {
        let status: SessionStatus = read_json(&paths.status)?;
        if status.state == SessionState::Completed {
            return Ok(status);
        }
        if status.state == SessionState::Failed {
            return Err(Error::InvalidControlState {
                action: ControlAction::Stop,
                state: status.state,
            });
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }
    Err(Error::ControlCompletionTimeout)
}

fn control_action_satisfied(action: ControlAction, state: SessionState) -> bool {
    matches!(
        (action, state),
        (ControlAction::Pause, SessionState::TranscriptionPaused)
            | (ControlAction::Resume, SessionState::Running)
            | (
                ControlAction::Stop,
                SessionState::Stopping | SessionState::Completed
            )
    )
}

fn current_session(sessions_dir: &Path) -> Result<(CurrentSession, SessionPaths)> {
    let sessions_dir = std::fs::canonicalize(sessions_dir).map_err(|error| Error::InvalidPath {
        label: "sessions directory",
        path: sessions_dir.to_path_buf(),
        reason: error.to_string(),
    })?;
    let current_path = sessions_dir.join(CURRENT_SESSION_FILE);
    if !current_path.exists() {
        return Err(Error::NoCurrentSession(sessions_dir.to_path_buf()));
    }
    let mut current: CurrentSession = read_json(&current_path)?;
    let candidate = if current.root.is_absolute() {
        current.root.clone()
    } else {
        let name = current
            .root
            .file_name()
            .ok_or_else(|| Error::InvalidSessionPath(current.root.clone()))?;
        sessions_dir.join(name)
    };
    let root = std::fs::canonicalize(&candidate)
        .map_err(|_| Error::InvalidSessionPath(candidate.clone()))?;
    let paths = SessionPaths::new(root.clone());
    if !paths.is_within(&sessions_dir) || root == sessions_dir {
        return Err(Error::InvalidSessionPath(root));
    }
    if !paths.metadata.is_file() || !paths.status.is_file() {
        return Err(Error::IncompleteSession(paths.root));
    }
    current.root = paths.root.clone();
    Ok((current, paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{SESSION_METADATA_FILE, STATUS_FILE};

    #[test]
    fn current_session_rejects_a_root_outside_the_sessions_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let sessions = temporary.path().join("sessions");
        let outside = temporary.path().join("outside");
        std::fs::create_dir_all(&sessions).expect("sessions directory should be created");
        std::fs::create_dir_all(&outside).expect("outside directory should be created");
        atomic_write_json(
            &sessions.join(CURRENT_SESSION_FILE),
            &CurrentSession {
                session_id: uuid::Uuid::nil(),
                root: outside,
            },
        )
        .expect("pointer should be written");

        assert!(matches!(
            current_session(&sessions),
            Err(Error::InvalidSessionPath(_))
        ));
    }

    #[test]
    fn legacy_relative_root_is_resolved_against_the_sessions_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let sessions = temporary.path().join("sessions");
        let root = sessions.join("session-1");
        std::fs::create_dir_all(&root).expect("session directory should be created");
        std::fs::write(root.join(SESSION_METADATA_FILE), b"{}")
            .expect("metadata placeholder should be written");
        std::fs::write(root.join(STATUS_FILE), b"{}")
            .expect("status placeholder should be written");
        atomic_write_json(
            &sessions.join(CURRENT_SESSION_FILE),
            &CurrentSession {
                session_id: uuid::Uuid::nil(),
                root: PathBuf::from("sessions/session-1"),
            },
        )
        .expect("pointer should be written");

        let (_, paths) = current_session(&sessions).expect("legacy pointer should resolve");
        assert_eq!(
            paths.root,
            std::fs::canonicalize(root).expect("root should canonicalize")
        );
    }
}
