use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
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
        serde_json::to_writer(&mut self.writer, segment)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

impl SessionStorage {
    pub fn create(sessions_dir: &Path, metadata: &SessionMetadata) -> Result<Self> {
        fs::create_dir_all(sessions_dir)?;
        let sessions_dir = fs::canonicalize(sessions_dir)?;
        let root = sessions_dir.join(format!(
            "{}-{}",
            metadata.started_at_unix_ms,
            &metadata.session_id.simple().to_string()[..8]
        ));
        fs::create_dir(&root)?;

        let paths = SessionPaths::new(root);
        fs::create_dir(&paths.audio)?;
        fs::create_dir(&paths.logs)?;
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
        serde_json::to_writer(&mut self.events, event)?;
        self.events.write_all(b"\n")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AudioSourceKind, PcmChunk, SessionConfig, SessionMetadata, SessionState, TimestampUs,
    };
    use crate::schema::SCHEMA_VERSION;

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
