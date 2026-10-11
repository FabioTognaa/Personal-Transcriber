use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::asr::{AsrEngine, WhisperEngine, validate_inference_config};
use crate::audio::{CompressedAudioSource, FileAudioSource};
use crate::domain::{InferenceConfig, PcmChunk, SegmenterConfig, SpeechSegment};
use crate::export::{SegmentLine, render_text};
use crate::segment::Segmenter;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub input: PathBuf,
    pub model: PathBuf,
    pub sessions_dir: PathBuf,
    pub language: String,
    pub segmenter: SegmenterConfig,
    pub inference: InferenceConfig,
}

/// One-shot transcription of an existing audio file. Unlike `start` it stores no
/// audio and creates no session: the transcript is written as `<stem>.txt` inside a
/// fresh, inert directory under `sessions_dir`, named after the day the run started.
pub fn run(options: TranscribeOptions) -> Result<()> {
    crate::session::validate_language(&options.language)?;
    validate_inference_config(&options.inference)?;
    crate::validation::validate_directory_target(&options.sessions_dir, "sessions directory")?;
    crate::validation::validate_existing_file(&options.input, "input audio")?;
    crate::validation::validate_existing_file(&options.model, "model")?;

    let started_at_unix_ms = crate::storage::unix_time_ms()?;
    let mut engine = WhisperEngine::load(&options.model, &options.inference)?;
    let mut source = AudioInput::open(&options.input)?;
    let mut segmenter = Segmenter::new(options.segmenter.clone())?;
    let lines = collect_lines(
        &mut source,
        &mut engine,
        &mut segmenter,
        &options.language,
        &options.inference,
    )?;

    // The directory is allocated only once the transcript is in hand, so a failure
    // above leaves nothing behind.
    std::fs::create_dir_all(&options.sessions_dir)?;
    let directory =
        crate::storage::create_session_directory(&options.sessions_dir, started_at_unix_ms)?;
    let txt_path = write_transcript(&directory, &options.input, &lines)?;
    println!("{}", txt_path.display());
    Ok(())
}

/// Writes `<stem>.txt` into `directory` and returns its path. Nothing else is written.
fn write_transcript(directory: &Path, input: &Path, lines: &[SegmentLine]) -> Result<PathBuf> {
    let mut path = directory.join(
        input
            .file_stem()
            .unwrap_or_else(|| OsStr::new("transcript")),
    );
    path.set_extension("txt");
    std::fs::write(&path, render_text(lines))?;
    Ok(path)
}

/// Decode, segment and transcribe in a single thread. Generic over the engine so
/// the loop can be tested without a real model.
fn collect_lines<E: AsrEngine>(
    source: &mut AudioInput,
    engine: &mut E,
    segmenter: &mut Segmenter,
    language: &str,
    inference: &InferenceConfig,
) -> Result<Vec<SegmentLine>> {
    let mut lines = Vec::new();
    while let Some(chunk) = source.next_chunk()? {
        append_segments(
            segmenter.push_chunk(chunk),
            engine,
            language,
            inference,
            &mut lines,
        )?;
    }
    append_segments(segmenter.flush(), engine, language, inference, &mut lines)?;
    Ok(lines)
}

fn append_segments<E: AsrEngine>(
    segments: Vec<SpeechSegment>,
    engine: &mut E,
    language: &str,
    inference: &InferenceConfig,
    lines: &mut Vec<SegmentLine>,
) -> Result<()> {
    for segment in segments {
        let output = engine.transcribe(&segment, language, inference)?;
        lines.push(SegmentLine {
            start: segment.start,
            end: segment.end,
            text: output.text,
        });
    }
    Ok(())
}

enum AudioInput {
    Wav {
        source: FileAudioSource,
        index: usize,
    },
    Compressed(CompressedAudioSource),
}

impl AudioInput {
    fn open(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            // ponytail: WAV goes through the in-memory `FileAudioSource`, so it
            // inherits its 64 MiB / 30 min cap. Stream WAV via `hound` if long WAVs
            // ever need `transcribe`.
            Some("wav") => Ok(Self::Wav {
                source: FileAudioSource::open(path)?,
                index: 0,
            }),
            Some("mp3" | "m4a" | "mp4" | "aac") => {
                Ok(Self::Compressed(CompressedAudioSource::open(path)?))
            }
            other => Err(Error::UnsupportedAudio(format!(
                "unsupported input format `{}`; supported: wav, mp3, m4a, mp4, aac",
                other.unwrap_or("(none)")
            ))),
        }
    }

    fn next_chunk(&mut self) -> Result<Option<PcmChunk>> {
        match self {
            Self::Wav { source, index } => {
                let chunk = source.chunk_at(*index);
                if chunk.is_some() {
                    *index += 1;
                }
                Ok(chunk)
            }
            Self::Compressed(source) => source.next_chunk(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::AsrOutput;
    use crate::domain::{SegmenterConfig, TimestampUs};

    struct MockAsr;

    impl AsrEngine for MockAsr {
        fn transcribe(
            &mut self,
            _segment: &SpeechSegment,
            _language: &str,
            _config: &InferenceConfig,
        ) -> Result<AsrOutput> {
            Ok(AsrOutput {
                text: "ciao".to_owned(),
                inference_duration_us: 1,
            })
        }
    }

    fn write_voiced_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("fixture WAV");
        for _ in 0..16_000 {
            writer.write_sample(i16::MAX / 4).expect("sample");
        }
        writer.finalize().expect("finalize");
    }

    #[test]
    fn collect_lines_segments_and_transcribes_a_wav() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let input = temporary.path().join("short.wav");
        write_voiced_wav(&input);

        let mut source = AudioInput::open(&input).expect("source opens");
        let mut engine = MockAsr;
        let mut segmenter = Segmenter::new(SegmenterConfig::default()).expect("segmenter");
        let lines = collect_lines(
            &mut source,
            &mut engine,
            &mut segmenter,
            "it",
            &InferenceConfig::default(),
        )
        .expect("transcription succeeds");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ciao");
        assert_eq!(lines[0].start, TimestampUs(0));
    }

    #[test]
    fn unsupported_extension_is_rejected_before_decoding() {
        let error = match AudioInput::open(Path::new("audio.flac")) {
            Err(error) => error,
            Ok(_) => panic!("flac is unsupported"),
        };
        assert!(matches!(error, Error::UnsupportedAudio(_)));
    }

    #[test]
    fn write_transcript_writes_only_the_txt_file() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let directory = temporary.path().join("26-10-11");
        std::fs::create_dir(&directory).expect("session dir");
        let lines = vec![SegmentLine {
            start: TimestampUs(0),
            end: TimestampUs(200_000),
            text: "ciao".to_owned(),
        }];

        let path = write_transcript(&directory, Path::new("/audio/meeting.wav"), &lines)
            .expect("transcript is written");

        assert_eq!(path, directory.join("meeting.txt"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("readable"),
            render_text(&lines)
        );
        let entries: Vec<_> = std::fs::read_dir(&directory)
            .expect("readable directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("meeting.txt")]);
    }

    #[test]
    fn failed_run_leaves_no_session_directory() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let sessions_dir = temporary.path().join("sessions");
        let input = temporary.path().join("meeting.wav");
        write_voiced_wav(&input);

        run(TranscribeOptions {
            input,
            model: temporary.path().join("missing-model.bin"),
            sessions_dir: sessions_dir.clone(),
            language: "it".to_owned(),
            segmenter: SegmenterConfig::default(),
            inference: InferenceConfig::default(),
        })
        .expect_err("a missing model must fail the run");

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
