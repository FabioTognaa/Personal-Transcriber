use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use uuid::Uuid;

use crate::domain::{PcmChunk, SessionEvent, SessionMetadata, SessionStatus};
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
}

impl SessionStorage {
    pub fn create(sessions_dir: &Path, metadata: &SessionMetadata) -> Result<Self> {
        fs::create_dir_all(sessions_dir)?;
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

        let storage = Self {
            paths,
            events,
            audio: Some(audio),
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
        let writer = self
            .audio
            .as_mut()
            .ok_or_else(|| Error::SessionFinalized(self.paths.root.clone()))?;
        for sample in &chunk.samples {
            let sample = sample.clamp(-1.0, 1.0);
            writer.write_sample((sample * f32::from(i16::MAX)).round() as i16)?;
        }
        writer.flush()?;
        Ok(())
    }

    pub fn finalize_audio(&mut self) -> Result<()> {
        if let Some(writer) = self.audio.take() {
            writer.finalize()?;
        }
        Ok(())
    }
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
    {
        let mut file = File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

pub fn unix_time_ms() -> Result<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::SystemClockBeforeUnixEpoch)?;
    u64::try_from(duration.as_millis()).map_err(|_| Error::TimestampOverflow)
}
