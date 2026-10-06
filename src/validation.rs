use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub const SESSION_DISK_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;
pub const DOCTOR_RECOMMENDED_FREE_BYTES: u64 = 1024 * 1024 * 1024;

pub fn validate_existing_file(path: &Path, label: &'static str) -> Result<u64> {
    let metadata = std::fs::metadata(path).map_err(|error| Error::InvalidPath {
        label,
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(Error::InvalidPath {
            label,
            path: path.to_path_buf(),
            reason: "expected a regular file".to_owned(),
        });
    }
    if metadata.len() == 0 {
        return Err(Error::InvalidPath {
            label,
            path: path.to_path_buf(),
            reason: "file is empty".to_owned(),
        });
    }
    Ok(metadata.len())
}

pub fn validate_directory_target(path: &Path, label: &'static str) -> Result<PathBuf> {
    if path.exists() {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        return Err(Error::InvalidPath {
            label,
            path: path.to_path_buf(),
            reason: "expected a directory".to_owned(),
        });
    }

    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    nearest_existing_ancestor(&resolved).ok_or_else(|| Error::InvalidPath {
        label,
        path: path.to_path_buf(),
        reason: "no existing parent directory".to_owned(),
    })
}

pub fn available_disk_space(path: &Path) -> Result<u64> {
    let existing = validate_directory_target(path, "sessions directory")?;
    available_disk_space_for_existing_path(&existing)
}

pub fn ensure_disk_space(path: &Path, required_bytes: u64) -> Result<u64> {
    let available_bytes = available_disk_space(path)?;
    if available_bytes < required_bytes {
        return Err(Error::InsufficientDiskSpace {
            path: path.to_path_buf(),
            required_bytes,
            available_bytes,
        });
    }
    Ok(available_bytes)
}

#[must_use]
pub fn estimated_session_bytes(sample_count: usize) -> u64 {
    let wav_data_bytes = u64::try_from(sample_count)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(std::mem::size_of::<i16>() as u16));
    wav_data_bytes.saturating_add(SESSION_DISK_HEADROOM_BYTES)
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.exists() && candidate.is_dir())
        .map(Path::to_path_buf)
}

#[cfg(unix)]
fn available_disk_space_for_existing_path(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let encoded = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::InvalidPath {
        label: "sessions directory",
        path: path.to_path_buf(),
        reason: "path contains a NUL byte".to_owned(),
    })?;
    let mut statistics = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `encoded` is NUL-terminated and `statistics` points to writable,
    // correctly aligned storage. A successful call initializes the structure.
    let result = unsafe { libc::statvfs(encoded.as_ptr(), statistics.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: statvfs returned success, so every field has been initialized.
    let statistics = unsafe { statistics.assume_init() };
    Ok((statistics.f_bavail as u64).saturating_mul(statistics.f_frsize))
}

#[cfg(not(unix))]
fn available_disk_space_for_existing_path(path: &Path) -> Result<u64> {
    Err(Error::InvalidPath {
        label: "sessions directory",
        path: path.to_path_buf(),
        reason: "free-space checks are not implemented on this platform".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_directory_uses_its_existing_parent_filesystem() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        let target = temporary.path().join("nested").join("sessions");

        assert!(available_disk_space(&target).expect("space should be measured") > 0);
    }

    #[test]
    fn nonexistent_relative_directory_uses_the_current_directory() {
        let target = PathBuf::from(format!("missing-{}", uuid::Uuid::new_v4())).join("sessions");

        let existing = validate_directory_target(&target, "sessions directory")
            .expect("relative target should resolve");

        assert_eq!(
            existing,
            std::env::current_dir().expect("current directory should be readable")
        );
    }

    #[test]
    fn regular_file_is_not_accepted_as_a_directory() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        let file = temporary.path().join("sessions");
        std::fs::write(&file, b"not a directory").expect("fixture should be written");

        assert!(matches!(
            available_disk_space(&file),
            Err(Error::InvalidPath {
                label: "sessions directory",
                ..
            })
        ));
    }

    #[test]
    fn impossible_space_requirement_is_rejected() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");

        assert!(matches!(
            ensure_disk_space(temporary.path(), u64::MAX),
            Err(Error::InsufficientDiskSpace { .. })
        ));
    }
}
