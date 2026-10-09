use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::asr::{AsrEngine, WhisperEngine, validate_inference_config};
use crate::audio::{CompressedAudioSource, FileAudioSource};
use crate::domain::{InferenceConfig, PcmChunk, SegmenterConfig, SpeechSegment};
use crate::export::{SegmentLine, render_markdown, render_text};
use crate::segment::Segmenter;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub input: PathBuf,
    pub model: PathBuf,
    pub language: String,
    pub segmenter: SegmenterConfig,
    pub inference: InferenceConfig,
}

/// One-shot transcription of an existing audio file. Unlike `start` it creates no
/// session directory and stores no audio: it writes `<stem>.txt` and `<stem>.md`
/// next to the input. Files are decoded concurrently with inference, so a long
/// recording finishes in decode + ASR time instead of real time.
pub fn run(options: TranscribeOptions) -> Result<()> {
    crate::session::validate_language(&options.language)?;
    validate_inference_config(&options.inference)?;
    crate::validation::validate_existing_file(&options.input, "input audio")?;

    let txt_path = options.input.with_extension("txt");
    let md_path = options.input.with_extension("md");
    for path in [&txt_path, &md_path] {
        if path.exists() {
            return Err(Error::OutputExists(path.clone()));
        }
    }

    crate::validation::validate_existing_file(&options.model, "model")?;
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

    let title = options
        .input
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("transcript");
    std::fs::write(&txt_path, render_text(&lines))?;
    std::fs::write(&md_path, render_markdown(&lines, title))?;
    println!("{}", txt_path.display());
    println!("{}", md_path.display());
    Ok(())
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
    fn existing_outputs_are_not_overwritten() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let input = temporary.path().join("meeting.wav");
        write_voiced_wav(&input);
        std::fs::write(temporary.path().join("meeting.txt"), b"existing").expect("output");

        let error = run(TranscribeOptions {
            input: input.clone(),
            model: temporary.path().join("missing-model.bin"),
            language: "it".to_owned(),
            segmenter: SegmenterConfig::default(),
            inference: InferenceConfig::default(),
        })
        .expect_err("existing output must be refused");

        assert!(
            matches!(error, Error::OutputExists(path) if path == temporary.path().join("meeting.txt"))
        );
    }
}
