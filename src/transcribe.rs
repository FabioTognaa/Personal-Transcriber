use std::ffi::OsStr;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::asr::{AsrEngine, WhisperEngine, contextual_prompt, validate_inference_config};
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
    let mut progress =
        ProgressBar::new(source.total_duration_us(), std::io::stdout().is_terminal());
    let lines = collect_lines(
        &mut source,
        &mut engine,
        &mut segmenter,
        &options.language,
        &options.inference,
        &mut |transcribed_us| progress.update(transcribed_us),
    )?;
    progress.finish();

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

const BAR_WIDTH: usize = 20;

/// Transcribed share of the total, 0-100. The numerator is the end of the last
/// segment fed to the ASR, not the decoded position: decoding is fast and runs
/// ahead, so decode progress would sit at 100% for most of the run.
fn percent_of(transcribed_us: u64, total_us: u64) -> u64 {
    if total_us == 0 {
        return 100;
    }
    transcribed_us
        .saturating_mul(100)
        .saturating_div(total_us)
        .min(100)
}

fn mmss(us: u64) -> String {
    let seconds = us / 1_000_000;
    let (hours, minutes, seconds) = (seconds / 3_600, (seconds % 3_600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
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
                mmss(transcribed_us.min(total)),
                mmss(total),
            )
        }
        None => format!(
            "[{:-<BAR_WIDTH$}] durata ignota  {} trascritti",
            "",
            mmss(transcribed_us),
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

/// Decode, segment and transcribe in a single thread. Generic over the engine so
/// the loop can be tested without a real model. `on_progress` receives the end of
/// each segment once it has been transcribed.
fn collect_lines<E: AsrEngine>(
    source: &mut AudioInput,
    engine: &mut E,
    segmenter: &mut Segmenter,
    language: &str,
    inference: &InferenceConfig,
    on_progress: &mut impl FnMut(u64),
) -> Result<Vec<SegmentLine>> {
    let mut lines = Vec::new();
    // Previous segment's text and confidence: the mobile prompt source when
    // context is enabled (same logic as the live sink).
    let mut previous: Option<(String, Option<f32>)> = None;
    while let Some(chunk) = source.next_chunk()? {
        append_segments(
            segmenter.push_chunk(chunk),
            engine,
            language,
            inference,
            &mut lines,
            on_progress,
            &mut previous,
        )?;
    }
    append_segments(
        segmenter.flush(),
        engine,
        language,
        inference,
        &mut lines,
        on_progress,
        &mut previous,
    )?;
    Ok(lines)
}

fn append_segments<E: AsrEngine>(
    segments: Vec<SpeechSegment>,
    engine: &mut E,
    language: &str,
    inference: &InferenceConfig,
    lines: &mut Vec<SegmentLine>,
    on_progress: &mut impl FnMut(u64),
    previous: &mut Option<(String, Option<f32>)>,
) -> Result<()> {
    for segment in segments {
        let end_us = segment.end.0;
        let (previous_text, previous_probability) = previous
            .as_ref()
            .map(|(text, probability)| (text.as_str(), *probability))
            .unwrap_or(("", None));
        let mut call_inference = inference.clone();
        call_inference.prompt =
            contextual_prompt(inference, previous_text, previous_probability);
        let output = engine.transcribe(&segment, language, &call_inference)?;
        *previous = Some((output.text.clone(), output.no_speech_probability));
        lines.push(SegmentLine {
            start: segment.start,
            end: segment.end,
            text: output.text,
            no_speech_probability: output.no_speech_probability,
        });
        on_progress(end_us);
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

    /// Total audio duration, when the source can tell without a scan.
    fn total_duration_us(&self) -> Option<u64> {
        match self {
            Self::Wav { source, .. } => {
                let samples = u64::try_from(source.sample_count()).ok()?;
                Some(
                    samples.saturating_mul(1_000_000)
                        / u64::from(crate::domain::TARGET_SAMPLE_RATE_HZ),
                )
            }
            Self::Compressed(source) => source.total_duration_us(),
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
                no_speech_probability: Some(0.1),
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
        let mut seen = Vec::new();
        let lines = collect_lines(
            &mut source,
            &mut engine,
            &mut segmenter,
            "it",
            &InferenceConfig::default(),
            &mut |end_us| seen.push(end_us),
        )
        .expect("transcription succeeds");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ciao");
        assert_eq!(lines[0].start, TimestampUs(0));
        // Progress reports the end of each transcribed segment.
        assert_eq!(seen, vec![lines[0].end.0]);
    }

    #[test]
    fn context_enabled_carries_the_previous_tail_into_the_next_prompt() {
        struct RecordingAsr {
            prompts: Vec<Option<String>>,
        }
        impl AsrEngine for RecordingAsr {
            fn transcribe(
                &mut self,
                _segment: &SpeechSegment,
                _language: &str,
                config: &InferenceConfig,
            ) -> Result<AsrOutput> {
                self.prompts.push(config.prompt.clone());
                Ok(AsrOutput {
                    text: "prima seconda".to_owned(),
                    inference_duration_us: 1,
                    no_speech_probability: Some(0.1),
                })
            }
        }

        fn speech_segment(start_us: u64) -> SpeechSegment {
            SpeechSegment {
                start: TimestampUs(start_us),
                end: TimestampUs(start_us + 200_000),
                samples: vec![0.25; 3_200],
            }
        }

        fn prompts_with(context_enabled: bool) -> Vec<Option<String>> {
            let inference = InferenceConfig {
                prompt: Some("base".to_owned()),
                context_enabled,
                ..InferenceConfig::default()
            };
            let mut engine = RecordingAsr { prompts: Vec::new() };
            let mut lines = Vec::new();
            let mut previous = None;
            append_segments(
                vec![speech_segment(0), speech_segment(2_000_000)],
                &mut engine,
                "it",
                &inference,
                &mut lines,
                &mut |_| {},
                &mut previous,
            )
            .expect("segments should transcribe");
            engine.prompts
        }

        // On: the second call receives the static prompt plus the tail of the
        // first segment's text. Off: both calls receive the static prompt only.
        assert_eq!(
            prompts_with(true),
            vec![
                Some("base".to_owned()),
                Some("base prima seconda".to_owned())
            ]
        );
        assert_eq!(
            prompts_with(false),
            vec![Some("base".to_owned()), Some("base".to_owned())]
        );
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
    fn mmss_widens_past_one_hour() {
        assert_eq!(mmss(0), "00:00");
        assert_eq!(mmss(59_000_000), "00:59");
        assert_eq!(mmss(3_600_000_000), "1:00:00");
        assert_eq!(mmss(5_425_000_000), "1:30:25");
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
            no_speech_probability: None,
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
