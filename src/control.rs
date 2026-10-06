use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::{ControlAction, ControlRequest, SessionState, SessionStatus};
use crate::schema::{CURRENT_SESSION_FILE, SCHEMA_VERSION, SessionPaths};
use crate::storage::{CurrentSession, atomic_write_json, read_json};
use crate::{Error, Result};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub fn current_status(sessions_dir: &Path) -> Result<SessionStatus> {
    let (_, paths) = current_session(sessions_dir)?;
    read_json(&paths.status)
}

pub fn request(sessions_dir: &Path, action: ControlAction) -> Result<SessionStatus> {
    let (_, paths) = current_session(sessions_dir)?;
    let status: SessionStatus = read_json(&paths.status)?;

    if status.state == SessionState::Completed {
        if action == ControlAction::Stop {
            return Ok(status);
        }
        return Err(Error::InvalidControlState {
            action,
            state: status.state,
        });
    }
    if status.state == SessionState::Stopping && action == ControlAction::Stop {
        return wait_for_completed(&paths);
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

    let deadline = Instant::now() + CONTROL_TIMEOUT;
    while Instant::now() < deadline {
        let updated: SessionStatus = read_json(&paths.status)?;
        if updated.applied_control_generation >= generation {
            if action == ControlAction::Stop {
                return wait_for_completed_until(&paths, deadline);
            }
            return Ok(updated);
        }
        if updated.state == SessionState::Completed && action == ControlAction::Stop {
            return Ok(updated);
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }
    Err(Error::ControlTimeout)
}

fn wait_for_completed(paths: &SessionPaths) -> Result<SessionStatus> {
    wait_for_completed_until(paths, Instant::now() + CONTROL_TIMEOUT)
}

fn wait_for_completed_until(paths: &SessionPaths, deadline: Instant) -> Result<SessionStatus> {
    while Instant::now() < deadline {
        let status: SessionStatus = read_json(&paths.status)?;
        if status.state == SessionState::Completed {
            return Ok(status);
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }
    Err(Error::ControlTimeout)
}

fn current_session(sessions_dir: &Path) -> Result<(CurrentSession, SessionPaths)> {
    let current_path = sessions_dir.join(CURRENT_SESSION_FILE);
    if !current_path.exists() {
        return Err(Error::NoCurrentSession(sessions_dir.to_path_buf()));
    }
    let current: CurrentSession = read_json(&current_path)?;
    let paths = SessionPaths::new(&current.root);
    Ok((current, paths))
}
