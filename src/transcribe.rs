use std::ffi::OsStr;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use uuid::Uuid;

use crate::asr::{
    Progress, load_engine, merge_timed_text, model_identity, validate_inference_config,
};
use crate::audio::DecodedAudioSource;
use crate::domain::{
    InferenceConfig, ModelIdentity, SegmenterConfig, SessionConfig, SessionMetadata, SessionState,
    TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ, TranscriptSegment, samples_to_us,
};
use crate::export::{SegmentLine, format_timestamp_short, render_text_with_timestamps};
use crate::model::ModelArtifact;
use crate::schema::{SCHEMA_VERSION, SESSION_METADATA_FILE, TRANSCRIPT_FILE};
use crate::storage::{TranscriptWriter, atomic_write_json, create_session_directory, unix_time_ms};
use crate::{Error, Result};

/// One batch call holds the whole decoded file in memory. Four hours is about
/// 880 MiB of samples; beyond that the error is clearer than an out-of-memory.
const MAX_BATCH_DURATION_SECS: u64 = 4 * 60 * 60;
const MAX_BATCH_DURATION_US: u64 = MAX_BATCH_DURATION_SECS * 1_000_000;

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub input: PathBuf,
    pub model: PathBuf,
    pub sessions_dir: PathBuf,
    pub language: String,
    pub inference: InferenceConfig,
    /// Repository name chosen in the start menu; the transcript directory becomes
    /// `YY-MM-DD_<name>`. `None` keeps the date-only name (non-interactive runs).
    pub session_name: Option<String>,
}

/// One-shot transcription of an existing audio file. It stores no audio, but it
/// writes a light session directory (`session.json`, `transcript.jsonl`,
/// `<stem>.txt`) so `export` can derive every representation from the canonical
/// transcript.
pub fn run(options: TranscribeOptions) -> Result<()> {
    crate::session::validate_language(&options.language)?;
    validate_inference_config(&options.inference)?;
    crate::validation::validate_directory_target(&options.sessions_dir, "sessions directory")?;
    crate::validation::validate_existing_file(&options.input, "input audio")?;
    validate_input_extension(&options.input)?;
    let artifact = ModelArtifact::detect(&options.model)?;
    let identity = model_identity(&artifact)?;

    // The batch path never retries through the temperature fallback: it is the
    // main fixed cost on silence and noise. The value is recorded in session.json.
    let mut inference = options.inference.clone();
    inference.temperature_inc = 0.0;
    validate_inference_config(&inference)?;

    let started_at_unix_ms = unix_time_ms()?;
    let mut source = DecodedAudioSource::open(&options.input)?;
    let total_us = source.total_duration_us();
    if let Some(total_us) = total_us {
        check_batch_duration(total_us)?;
    }

    let mut samples = Vec::new();
    while let Some(chunk) = source.next_chunk()? {
        samples.extend_from_slice(&chunk.samples);
        check_batch_duration(samples_to_us(samples.len()))?;
    }

    let mut engine = load_engine(&artifact, &inference)?;
    let progress_bar = Arc::new(Mutex::new(ProgressBar::new(
        total_us,
        std::io::stdout().is_terminal(),
    )));
    let progress = Progress::new({
        let progress_bar = Arc::clone(&progress_bar);
        move |transcribed_us| {
            progress_bar
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .update(transcribed_us);
        }
    });
    let timed = engine.transcribe_batch(&samples, &options.language, &inference, &progress);
    progress_bar
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .finish();
    let lines = merge_timed_text(timed?)
        .into_iter()
        .map(|piece| SegmentLine {
            start: piece.start,
            end: piece.end,
            text: piece.text,
        })
        .collect::<Vec<_>>();

    // The directory is allocated only once the transcript is in hand, so a failure
    // above leaves nothing behind.
    std::fs::create_dir_all(&options.sessions_dir)?;
    let directory = create_session_directory(
        &options.sessions_dir,
        started_at_unix_ms,
        options.session_name.as_deref(),
    )?;
    write_or_remove(&directory, || {
        write_transcript_directory(
            &directory,
            &options,
            &identity,
            &inference,
            &lines,
            started_at_unix_ms,
        )
    })?;
    println!("{}", directory.display());
    Ok(())
}

/// Runs the output writer on a freshly allocated directory and removes it if the
/// write fails, so a failed run leaves nothing behind.
fn write_or_remove(directory: &Path, write: impl FnOnce() -> Result<()>) -> Result<()> {
    if let Err(error) = write() {
        let _ = std::fs::remove_dir_all(directory);
        return Err(error);
    }
    Ok(())
}

fn validate_input_extension(path: &Path) -> Result<()> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("wav" | "mp3" | "m4a" | "mp4" | "aac") => Ok(()),
        other => Err(Error::UnsupportedAudio(format!(
            "unsupported input format `{}`; supported: wav, mp3, m4a, mp4, aac",
            other.unwrap_or("(none)")
        ))),
    }
}

fn check_batch_duration(duration_us: u64) -> Result<()> {
    if duration_us > MAX_BATCH_DURATION_US {
        return Err(Error::AudioTooLong {
            duration_secs: duration_us / 1_000_000,
            limit_secs: MAX_BATCH_DURATION_SECS,
        });
    }
    Ok(())
}

/// Writes the canonical JSONL, the timestamped `.txt` and `session.json` into a
/// directory that is already allocated. No audio, status or control files.
fn write_transcript_directory(
    directory: &Path,
    options: &TranscribeOptions,
    identity: &ModelIdentity,
    inference: &InferenceConfig,
    lines: &[SegmentLine],
    started_at_unix_ms: u64,
) -> Result<()> {
    let session_id = Uuid::new_v4();
    let mut writer = TranscriptWriter::open(&directory.join(TRANSCRIPT_FILE))?;
    for line in lines {
        writer.append(&TranscriptSegment {
            schema_version: SCHEMA_VERSION,
            segment_id: Uuid::new_v4(),
            session_id,
            start: line.start,
            end: line.end,
            text: line.text.clone(),
            language: options.language.clone(),
            model: identity.name.clone(),
            model_sha256: identity.sha256.clone(),
            engine: identity.engine,
            inference: inference.clone(),
        })?;
    }
    write_transcript(directory, &options.input, lines)?;

    let metadata = SessionMetadata {
        schema_version: SCHEMA_VERSION,
        session_id,
        state: SessionState::Completed,
        started_at_unix_ms,
        ended_at_unix_ms: Some(unix_time_ms()?),
        config: SessionConfig {
            language: options.language.clone(),
            model_path: Some(options.model.clone()),
            model: Some(identity.clone()),
            file_source: Some(options.input.clone()),
            microphone_device: None,
            system_device: None,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
            silence_timeout_ms: None,
            // The schema requires it; the batch path does not use the live VAD.
            segmenter: SegmenterConfig::default(),
            inference: inference.clone(),
        },
    };
    atomic_write_json(&directory.join(SESSION_METADATA_FILE), &metadata)?;
    Ok(())
}

/// Writes `<stem>.txt` into `directory` and returns its path.
fn write_transcript(directory: &Path, input: &Path, lines: &[SegmentLine]) -> Result<PathBuf> {
    let mut path = directory.join(
        input
            .file_stem()
            .unwrap_or_else(|| OsStr::new("transcript")),
    );
    path.set_extension("txt");
    std::fs::write(&path, render_text_with_timestamps(lines))?;
    Ok(path)
}

const BAR_WIDTH: usize = 20;

/// Transcribed share of the total, 0-100. The numerator is the end of the last
/// timed piece reported by the engine, not the decoded position.
fn percent_of(transcribed_us: u64, total_us: u64) -> u64 {
    if total_us == 0 {
        return 100;
    }
    transcribed_us
        .saturating_mul(100)
        .saturating_div(total_us)
        .min(100)
}

/// One progress line: a bar, the percentage of the transcription done, and the
/// transcribed/total time. Never an estimate of what is left.
fn progress_line(transcribed_us: u64, total_us: Option<u64>) -> String {
    match total_us.filter(|total| *total > 0) {
        Some(total) => {
            let percent = percent_of(transcribed_us, total);
            let done = (percent as usize * BAR_WIDTH / 100).min(BAR_WIDTH);
            format!(
                "[{}{}] {percent:>3}%  {} / {}",
                "#".repeat(done),
                "-".repeat(BAR_WIDTH - done),
                format_timestamp_short(transcribed_us.min(total)),
                format_timestamp_short(total),
            )
        }
        None => format!(
            "[{:-<BAR_WIDTH$}] durata ignota  {} trascritti",
            "",
            format_timestamp_short(transcribed_us),
        ),
    }
}

/// Single-line progress on stdout, redrawn in place. Prints only when the line
/// changes (at most ~1/s), and nothing at all when stdout is not a terminal.
struct ProgressBar {
    total_us: Option<u64>,
    last_line: Option<String>,
    live: bool,
}

impl ProgressBar {
    fn new(total_us: Option<u64>, live: bool) -> Self {
        Self {
            total_us,
            last_line: None,
            live,
        }
    }

    fn update(&mut self, transcribed_us: u64) {
        if !self.live {
            return;
        }
        let line = progress_line(transcribed_us, self.total_us);
        if self.last_line.as_deref() == Some(line.as_str()) {
            return;
        }
        print!("\r\x1b[K{line}");
        let _ = std::io::stdout().flush();
        self.last_line = Some(line);
    }

    /// Erases the line, so the caller's final output starts on a clean row.
    fn finish(&mut self) {
        if self.live && self.last_line.is_some() {
            print!("\r\x1b[K");
            let _ = std::io::stdout().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{EngineKind, TimestampUs};
    use std::collections::BTreeSet;

    fn identity() -> ModelIdentity {
        ModelIdentity {
            path: PathBuf::from("models/fixture.bin"),
            name: "fixture.bin".to_owned(),
            sha256: "fixture-sha256".to_owned(),
            size_bytes: 1,
            engine: EngineKind::Whisper,
        }
    }

    fn options(input: PathBuf, sessions_dir: PathBuf) -> TranscribeOptions {
        TranscribeOptions {
            input,
            model: PathBuf::from("models/fixture.bin"),
            sessions_dir,
            language: "it".to_owned(),
            inference: InferenceConfig::default(),
            session_name: None,
        }
    }

    #[test]
    fn progress_line_shows_the_percentage_transcribed_not_the_time_left() {
        assert_eq!(
            progress_line(0, Some(10_000_000)),
            "[--------------------]   0%  00:00 / 00:10"
        );
        assert_eq!(
            progress_line(5_000_000, Some(10_000_000)),
            "[##########----------]  50%  00:05 / 00:10"
        );
        assert_eq!(
            progress_line(20_000_000, Some(10_000_000)),
            "[####################] 100%  00:10 / 00:10"
        );
        assert_eq!(
            progress_line(3_725_000, Some(7_450_000)),
            "[##########----------]  50%  00:03 / 00:07"
        );
    }

    #[test]
    fn progress_line_falls_back_to_indeterminate_without_a_duration() {
        assert_eq!(
            progress_line(65_000_000, None),
            "[--------------------] durata ignota  01:05 trascritti"
        );
        assert_eq!(
            progress_line(1, Some(0)),
            "[--------------------] durata ignota  00:00 trascritti"
        );
    }

    #[test]
    fn batch_guard_rejects_audio_over_four_hours() {
        assert!(check_batch_duration(MAX_BATCH_DURATION_US).is_ok());
        assert!(matches!(
            check_batch_duration(MAX_BATCH_DURATION_US + 1),
            Err(Error::AudioTooLong { .. })
        ));
    }

    #[test]
    fn unsupported_extension_is_rejected_before_decoding() {
        let error =
            validate_input_extension(Path::new("audio.flac")).expect_err("flac is unsupported");
        assert!(matches!(error, Error::UnsupportedAudio(_)));
    }

    #[test]
    fn writes_canonical_transcript_and_timestamped_txt() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let directory = temporary.path().join("26-10-11");
        std::fs::create_dir(&directory).expect("session dir");
        let input = temporary.path().join("meeting.wav");
        let options = options(input.clone(), temporary.path().to_path_buf());
        let lines = vec![SegmentLine {
            start: TimestampUs(0),
            end: TimestampUs(7_000_000),
            text: "ciao".to_owned(),
        }];

        write_transcript_directory(
            &directory,
            &options,
            &identity(),
            &options.inference,
            &lines,
            1,
        )
        .expect("transcript directory is written");

        let txt = std::fs::read_to_string(directory.join("meeting.txt")).expect("txt");
        assert_eq!(txt, "[00:00 → 00:07] ciao\n");

        let transcript = std::fs::read_to_string(directory.join(TRANSCRIPT_FILE)).expect("jsonl");
        let segment: TranscriptSegment =
            serde_json::from_str(transcript.trim()).expect("canonical record");
        assert_eq!(segment.text, "ciao");
        assert_eq!(segment.start, TimestampUs(0));
        assert_eq!(segment.end, TimestampUs(7_000_000));
        assert_eq!(segment.engine, EngineKind::Whisper);

        let metadata: SessionMetadata = serde_json::from_slice(
            &std::fs::read(directory.join(SESSION_METADATA_FILE)).expect("session.json"),
        )
        .expect("metadata");
        assert_eq!(metadata.state, SessionState::Completed);
        assert_eq!(metadata.config.file_source, Some(input));
        assert!(metadata.config.model.is_some());

        let entries: BTreeSet<_> = std::fs::read_dir(&directory)
            .expect("readable directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        let expected: BTreeSet<_> = [
            "meeting.txt".into(),
            SESSION_METADATA_FILE.into(),
            TRANSCRIPT_FILE.into(),
        ]
        .into_iter()
        .collect();
        assert_eq!(entries, expected);
    }

    #[test]
    fn failed_output_write_removes_the_new_directory() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let directory = temporary.path().join("session");
        std::fs::create_dir(&directory).expect("session dir");

        let result = write_or_remove(&directory, || {
            Err(Error::InvalidExport("simulated write failure".to_owned()))
        });

        assert!(result.is_err());
        assert!(!directory.exists(), "the partial directory must be removed");
    }

    #[test]
    fn failed_run_leaves_no_session_directory() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let sessions_dir = temporary.path().join("sessions");
        let input = temporary.path().join("meeting.wav");
        std::fs::write(&input, b"not decoded because the model is missing").expect("input");

        run(options(input, sessions_dir.clone())).expect_err("a missing model must fail the run");

        let leftover = if sessions_dir.exists() {
            std::fs::read_dir(&sessions_dir)
                .expect("readable directory")
                .count()
        } else {
            0
        };
        assert_eq!(leftover, 0, "no session directory should be left behind");
    }
}
