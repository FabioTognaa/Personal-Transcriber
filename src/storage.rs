use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use uuid::Uuid;

use crate::domain::{PcmChunk, SessionEvent, SessionMetadata, SessionStatus, TranscriptSegment};
use crate::schema::{CURRENT_SESSION_FILE, SessionPaths};
use crate::{Error, Result};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CurrentSession {
    pub session_id: Uuid,
    pub root: PathBuf,
}

pub struct SessionStorage {
    pub paths: SessionPaths,
    events: BufWriter<File>,
    audio: Option<hound::WavWriter<BufWriter<File>>>,
    microphone_audio: Option<hound::WavWriter<BufWriter<File>>>,
    system_audio: Option<hound::WavWriter<BufWriter<File>>>,
}

pub struct TranscriptWriter {
    writer: BufWriter<File>,
}

pub struct ExclusiveFileLock {
    file: File,
}

impl ExclusiveFileLock {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: `file` owns a valid descriptor for the duration of the lock.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Some(Self { file }));
            }
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
            ) {
                return Ok(None);
            }
            Err(error.into())
        }
        #[cfg(not(unix))]
        {
            Ok(Some(Self { file }))
        }
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the descriptor remains valid until `file` is dropped.
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

impl TranscriptWriter {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            writer: append_file(path)?,
        })
    }

    pub fn append(&mut self, segment: &TranscriptSegment) -> Result<()> {
        write_json_line(&mut self.writer, segment)?;
        self.writer.flush()?;
        Ok(())
    }
}

impl SessionStorage {
    pub fn create(sessions_dir: &Path, metadata: &SessionMetadata) -> Result<Self> {
        fs::create_dir_all(sessions_dir)?;
        let sessions_dir = fs::canonicalize(sessions_dir)?;
        let root = create_session_directory(&sessions_dir, metadata.started_at_unix_ms)?;

        let paths = SessionPaths::new(root);
        fs::create_dir(&paths.audio)?;
        File::create(&paths.transcript)?;
        let events = append_file(&paths.events)?;

        let spec = hound::WavSpec {
            channels: crate::domain::TARGET_CHANNELS,
            sample_rate: crate::domain::TARGET_SAMPLE_RATE_HZ,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let audio = hound::WavWriter::create(&paths.mixed_audio, spec)?;
        let live = metadata.config.file_source.is_none();
        let microphone_audio = live
            .then(|| hound::WavWriter::create(&paths.microphone_audio, spec))
            .transpose()?;
        let system_audio = live
            .then(|| hound::WavWriter::create(&paths.system_audio, spec))
            .transpose()?;

        let storage = Self {
            paths,
            events,
            audio: Some(audio),
            microphone_audio,
            system_audio,
        };
        storage.write_metadata(metadata)?;
        atomic_write_json(
            &sessions_dir.join(CURRENT_SESSION_FILE),
            &CurrentSession {
                session_id: metadata.session_id,
                root: storage.paths.root.clone(),
            },
        )?;

        Ok(storage)
    }

    pub fn write_metadata(&self, metadata: &SessionMetadata) -> Result<()> {
        atomic_write_json(&self.paths.metadata, metadata)
    }

    pub fn write_status(&self, status: &SessionStatus) -> Result<()> {
        atomic_write_json(&self.paths.status, status)
    }

    pub fn append_event(&mut self, event: &SessionEvent) -> Result<()> {
        write_json_line(&mut self.events, event)?;
        self.events.flush()?;
        Ok(())
    }

    pub fn write_audio(&mut self, chunk: &PcmChunk) -> Result<()> {
        write_chunk(
            self.audio
                .as_mut()
                .ok_or_else(|| Error::SessionFinalized(self.paths.root.clone()))?,
            chunk,
        )
    }

    pub fn write_live_audio(
        &mut self,
        microphone: &PcmChunk,
        system: &PcmChunk,
        mixed: &PcmChunk,
    ) -> Result<()> {
        let root = self.paths.root.clone();
        write_chunk(
            self.microphone_audio
                .as_mut()
                .ok_or_else(|| Error::SessionFinalized(root.clone()))?,
            microphone,
        )?;
        write_chunk(
            self.system_audio
                .as_mut()
                .ok_or(Error::SessionFinalized(root))?,
            system,
        )?;
        self.write_audio(mixed)
    }

    pub fn finalize_audio(&mut self) -> Result<()> {
        let mut first_error = None;
        for writer in [
            &mut self.microphone_audio,
            &mut self.system_audio,
            &mut self.audio,
        ] {
            if let Err(error) = finalize_writer(writer)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn write_chunk(writer: &mut hound::WavWriter<BufWriter<File>>, chunk: &PcmChunk) -> Result<()> {
    for sample in &chunk.samples {
        let sample = sample.clamp(-1.0, 1.0);
        writer.write_sample((sample * f32::from(i16::MAX)).round() as i16)?;
    }
    writer.flush()?;
    Ok(())
}

fn finalize_writer(writer: &mut Option<hound::WavWriter<BufWriter<File>>>) -> Result<()> {
    if let Some(writer) = writer.take() {
        writer.finalize()?;
    }
    Ok(())
}

fn append_file(path: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        OpenOptions::new().create(true).append(true).open(path)?,
    ))
}

fn write_json_line(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    Ok(())
}

pub fn repair_jsonl_tail(path: &Path) -> Result<bool> {
    let contents = fs::read(path)?;
    if contents.is_empty() || contents.ends_with(b"\n") {
        return Ok(false);
    }

    let tail_start = contents
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let tail = &contents[tail_start..];
    if serde_json::from_slice::<serde_json::Value>(tail).is_ok() {
        let mut file = OpenOptions::new().append(true).open(path)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    } else {
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(tail_start as u64)?;
    }
    Ok(true)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_reader(File::open(path)?)?)
}

pub fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidSessionPath(path.to_path_buf()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn unix_time_ms() -> Result<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::SystemClockBeforeUnixEpoch)?;
    u64::try_from(duration.as_millis()).map_err(|_| Error::TimestampOverflow)
}

pub(crate) fn session_directory_name(started_at_unix_ms: u64) -> Result<String> {
    let seconds =
        i64::try_from(started_at_unix_ms / 1_000).map_err(|_| Error::TimestampOverflow)?;
    let tm = local_broken_down_time(seconds)?;
    let year = tm
        .tm_year
        .checked_add(1900)
        .ok_or(Error::TimestampOverflow)?;
    let month = tm.tm_mon.checked_add(1).ok_or(Error::TimestampOverflow)?;
    Ok(format!(
        "{year:04}-{month:02}-{mday:02}_{hour:02}-{minute:02}-{second:02}",
        mday = tm.tm_mday,
        hour = tm.tm_hour,
        minute = tm.tm_min,
        second = tm.tm_sec,
    ))
}

fn create_session_directory(sessions_dir: &Path, started_at_unix_ms: u64) -> Result<PathBuf> {
    let base = session_directory_name(started_at_unix_ms)?;
    for attempt in 1u32..=1_000 {
        let name = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}_{attempt}")
        };
        let root = sessions_dir.join(name);
        match fs::create_dir(&root) {
            Ok(()) => return Ok(root),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::InvalidPath {
        label: "session",
        path: sessions_dir.join(&base),
        reason: "could not allocate a unique session directory name".to_owned(),
    })
}

fn local_broken_down_time(seconds: i64) -> Result<libc::tm> {
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `tm` is a valid `tm` buffer and remains owned by this function.
    let pointer = unsafe { libc::localtime_r(&seconds, tm.as_mut_ptr()) };
    if pointer.is_null() {
        return Err(Error::TimestampOverflow);
    }
    // SAFETY: `localtime_r` initialized `tm` before returning a non-null pointer.
    Ok(unsafe { tm.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AudioSourceKind, PcmChunk, SessionConfig, SessionMetadata, SessionState, TimestampUs,
    };
    use crate::schema::SCHEMA_VERSION;

    #[test]
    fn session_directory_name_matches_local_start_timestamp() {
        let name = session_directory_name(1_760_000_000_000).expect("name should format");
        assert!(
            is_session_directory_name(&name),
            "unexpected session directory name {name}"
        );
    }

    #[test]
    fn session_directory_uses_local_start_and_avoids_collisions() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let metadata = file_metadata();
        let first = SessionStorage::create(temporary.path(), &metadata).expect("first session");
        let second = SessionStorage::create(temporary.path(), &metadata).expect("second session");
        let first_name = first
            .paths
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .expect("first name");
        let second_name = second
            .paths
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .expect("second name");

        assert!(is_session_directory_name(first_name));
        assert_eq!(second_name, format!("{first_name}_2"));
        assert!(!first.paths.root.join("logs").exists());
    }

    #[test]
    fn live_storage_persists_both_original_tracks_and_the_mix() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let mut config = SessionConfig::italian(None);
        config.microphone_device = Some("Test microphone".to_owned());
        config.system_device = Some("BlackHole 2ch".to_owned());
        let metadata = SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: Uuid::new_v4(),
            state: SessionState::Starting,
            started_at_unix_ms: 1,
            ended_at_unix_ms: None,
            config,
        };
        let mut storage =
            SessionStorage::create(temporary.path(), &metadata).expect("storage should open");
        storage
            .write_live_audio(
                &chunk(AudioSourceKind::Microphone, 0.25),
                &chunk(AudioSourceKind::System, 0.5),
                &chunk(AudioSourceKind::Mixed, 0.375),
            )
            .expect("all tracks should be written");
        storage.finalize_audio().expect("WAVs should finalize");

        for path in [
            &storage.paths.microphone_audio,
            &storage.paths.system_audio,
            &storage.paths.mixed_audio,
        ] {
            assert_eq!(
                hound::WavReader::open(path)
                    .expect("track should be readable")
                    .duration(),
                320
            );
        }
    }

    #[test]
    fn exclusive_file_lock_is_recoverable_after_owner_drop() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let path = temporary.path().join("runner.lock");
        let first = ExclusiveFileLock::try_acquire(&path)
            .expect("lock attempt should succeed")
            .expect("first owner should acquire the lock");
        assert!(
            ExclusiveFileLock::try_acquire(&path)
                .expect("competing lock attempt should be readable")
                .is_none()
        );
        drop(first);
        assert!(
            ExclusiveFileLock::try_acquire(&path)
                .expect("lock should be reusable")
                .is_some()
        );
    }

    #[test]
    fn current_session_always_stores_an_absolute_root() {
        let relative = PathBuf::from("target").join(format!("sessions-{}", Uuid::new_v4()));
        let metadata = SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: Uuid::new_v4(),
            state: SessionState::Starting,
            started_at_unix_ms: 1,
            ended_at_unix_ms: None,
            config: SessionConfig::italian(Some(PathBuf::from("fixture.wav"))),
        };
        let storage = SessionStorage::create(&relative, &metadata)
            .expect("relative sessions directory should work");
        let current: CurrentSession = read_json(&relative.join(CURRENT_SESSION_FILE))
            .expect("current session should be readable");

        assert!(current.root.is_absolute());
        drop(storage);
        std::fs::remove_dir_all(&relative).expect("test directory should be removed");
    }

    #[test]
    fn jsonl_tail_repair_completes_valid_records_and_discards_partial_ones() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let valid = temporary.path().join("valid.jsonl");
        let partial = temporary.path().join("partial.jsonl");
        fs::write(&valid, b"{\"complete\":true}").expect("valid fixture should be written");
        fs::write(&partial, b"{\"first\":true}\n{\"incomplete\"")
            .expect("partial fixture should be written");

        assert!(repair_jsonl_tail(&valid).expect("valid tail should be repaired"));
        assert!(repair_jsonl_tail(&partial).expect("partial tail should be repaired"));
        assert_eq!(
            fs::read_to_string(valid).expect("valid fixture should be readable"),
            "{\"complete\":true}\n"
        );
        assert_eq!(
            fs::read_to_string(partial).expect("partial fixture should be readable"),
            "{\"first\":true}\n"
        );
    }

    fn file_metadata() -> SessionMetadata {
        SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: Uuid::new_v4(),
            state: SessionState::Starting,
            started_at_unix_ms: 1,
            ended_at_unix_ms: None,
            config: SessionConfig::italian(Some(PathBuf::from("fixture.wav"))),
        }
    }

    fn is_session_directory_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        bytes.len() == 19
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'_'
            && bytes[13] == b'-'
            && bytes[16] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit())
    }

    fn chunk(source: AudioSourceKind, value: f32) -> PcmChunk {
        PcmChunk {
            source,
            start: TimestampUs(0),
            sample_rate_hz: crate::domain::TARGET_SAMPLE_RATE_HZ,
            channels: crate::domain::TARGET_CHANNELS,
            samples: vec![value; 320],
        }
    }
}
