use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::domain::{ControlAction, InferenceConfig, InferenceStrategy, SegmenterConfig};
use crate::session::StartOptions;
use crate::{Error, Result};

#[derive(Debug, Parser)]
#[command(
    name = "live-transcript",
    version,
    about = "Record and transcribe live meetings locally"
)]
pub struct Cli {
    /// Increase diagnostic verbosity. Repeat for more detail.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Directory under which session directories are created.
    #[arg(long, default_value = "sessions", global = true)]
    pub sessions_dir: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List audio devices visible to the application.
    Devices,
    /// Check local prerequisites without changing system configuration.
    Doctor(DoctorArgs),
    /// Start a local recording and transcription session.
    Start(StartArgs),
    /// Show the current session state and backlog.
    Status,
    /// Pause transcription while audio recording continues.
    Pause,
    /// Resume transcription for the active session.
    Resume,
    /// Finalize the active session.
    Stop,
    /// Export a completed canonical transcript.
    Export(ExportArgs),
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Local whisper.cpp GGML model to validate.
    #[arg(long, default_value = "models/ggml-small-q5_1.bin")]
    pub model: PathBuf,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// WAV file replayed in real time by the M1 simulator.
    #[arg(long)]
    pub input_wav: PathBuf,

    /// Local whisper.cpp GGML model path.
    #[arg(long)]
    pub model: PathBuf,

    /// Input device used for the local microphone.
    #[arg(long)]
    pub microphone: Option<String>,

    /// Input device used for remote/system audio, normally BlackHole on macOS.
    #[arg(long)]
    pub system_audio: Option<String>,

    /// Spoken language. The v1 implementation supports Italian only.
    #[arg(long, default_value = "it")]
    pub language: String,

    /// RMS threshold above which a PCM chunk is treated as voice.
    #[arg(long, default_value_t = 0.02)]
    pub vad_threshold: f32,

    /// Consecutive voice required to start a segment.
    #[arg(long, default_value_t = 60)]
    pub vad_start_ms: u64,

    /// Consecutive silence required to end a segment.
    #[arg(long, default_value_t = 800)]
    pub vad_end_ms: u64,

    /// Audio retained before detected speech.
    #[arg(long, default_value_t = 200)]
    pub vad_pre_roll_ms: u64,

    /// Audio retained after the last detected speech.
    #[arg(long, default_value_t = 200)]
    pub vad_post_roll_ms: u64,

    /// Minimum voiced duration accepted as a segment.
    #[arg(long, default_value_t = 100)]
    pub vad_min_speech_ms: u64,

    /// Maximum segment duration before a deterministic split.
    #[arg(long, default_value_t = 30_000)]
    pub vad_max_segment_ms: u64,

    /// CPU threads used by whisper.cpp.
    #[arg(long, default_value_t = default_threads())]
    pub asr_threads: i32,

    /// Whisper decoding strategy.
    #[arg(long, value_enum, default_value_t = DecoderStrategy::BeamSearch)]
    pub asr_strategy: DecoderStrategy,

    /// Candidate count for greedy decoding.
    #[arg(long, default_value_t = 5)]
    pub asr_best_of: i32,

    /// Beam width for beam-search decoding.
    #[arg(long, default_value_t = 5)]
    pub asr_beam_size: i32,

    /// Initial decoding temperature.
    #[arg(long, default_value_t = 0.0)]
    pub asr_temperature: f32,

    /// Enable whisper.cpp flash attention.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub asr_flash_attention: bool,

    /// Require a binary built with the optional Core ML feature.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub asr_coreml: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DecoderStrategy {
    Greedy,
    BeamSearch,
}

impl From<DecoderStrategy> for InferenceStrategy {
    fn from(strategy: DecoderStrategy) -> Self {
        match strategy {
            DecoderStrategy::Greedy => Self::Greedy,
            DecoderStrategy::BeamSearch => Self::BeamSearch,
        }
    }
}

fn default_threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |count| count.get().min(8) as i32)
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Session directory containing transcript.jsonl.
    pub session: PathBuf,

    /// Output representation derived from the canonical JSONL transcript.
    #[arg(long, value_enum, default_value_t = ExportFormat::Markdown)]
    pub format: ExportFormat,

    /// Destination file. Standard output is used when omitted.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Jsonl,
    Text,
    Markdown,
    Srt,
    Vtt,
}

pub fn execute(command: Command, sessions_dir: PathBuf) -> Result<()> {
    match command {
        Command::Devices => {
            print_json(&crate::diagnostics::audio_devices()?)?;
            Ok(())
        }
        Command::Doctor(args) => {
            let report = crate::diagnostics::doctor(&sessions_dir, &args.model);
            print_json(&report)?;
            if report.ready {
                Ok(())
            } else {
                Err(Error::DoctorFailed)
            }
        }
        Command::Start(args) => {
            let root = crate::session::run(StartOptions {
                sessions_dir,
                input_wav: args.input_wav,
                model: Some(args.model),
                language: args.language,
                microphone: args.microphone,
                system_audio: args.system_audio,
                segmenter: SegmenterConfig {
                    energy_threshold: args.vad_threshold,
                    start_trigger_ms: args.vad_start_ms,
                    end_silence_ms: args.vad_end_ms,
                    pre_roll_ms: args.vad_pre_roll_ms,
                    post_roll_ms: args.vad_post_roll_ms,
                    min_speech_ms: args.vad_min_speech_ms,
                    max_segment_ms: args.vad_max_segment_ms,
                },
                inference: InferenceConfig {
                    threads: args.asr_threads,
                    strategy: args.asr_strategy.into(),
                    best_of: args.asr_best_of,
                    beam_size: args.asr_beam_size,
                    temperature: args.asr_temperature,
                    flash_attention: args.asr_flash_attention,
                    coreml: args.asr_coreml,
                },
                model_identity: None,
            })?;
            println!("{}", root.display());
            Ok(())
        }
        Command::Status => {
            print_json(&crate::control::status_report(&sessions_dir)?)?;
            Ok(())
        }
        Command::Pause => {
            print_status(&crate::control::request(
                &sessions_dir,
                ControlAction::Pause,
            )?)?;
            Ok(())
        }
        Command::Resume => {
            print_status(&crate::control::request(
                &sessions_dir,
                ControlAction::Resume,
            )?)?;
            Ok(())
        }
        Command::Stop => {
            print_status(&crate::control::request(
                &sessions_dir,
                ControlAction::Stop,
            )?)?;
            Ok(())
        }
        Command::Export(args) => {
            crate::export::export_session(
                &args.session,
                args.format.into(),
                args.output.as_deref(),
            )?;
            Ok(())
        }
    }
}

impl From<ExportFormat> for crate::export::Format {
    fn from(format: ExportFormat) -> Self {
        match format {
            ExportFormat::Jsonl => Self::Jsonl,
            ExportFormat::Text => Self::Text,
            ExportFormat::Markdown => Self::Markdown,
            ExportFormat::Srt => Self::Srt,
            ExportFormat::Vtt => Self::Vtt,
        }
    }
}

fn print_status(status: &crate::domain::SessionStatus) -> Result<()> {
    print_json(status)
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn start_defaults_to_italian() {
        let cli = Cli::try_parse_from([
            "live-transcript",
            "start",
            "--input-wav",
            "tests/fixtures/m1_stream.wav",
            "--model",
            "models/fixture.bin",
        ])
        .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.language, "it");
        assert_eq!(
            args.input_wav,
            PathBuf::from("tests/fixtures/m1_stream.wav")
        );
        assert_eq!(args.model, PathBuf::from("models/fixture.bin"));
        assert_eq!(cli.sessions_dir, PathBuf::from("sessions"));
    }

    #[test]
    fn export_format_parses_from_cli() {
        let cli = Cli::try_parse_from([
            "live-transcript",
            "export",
            "sessions/example",
            "--format",
            "vtt",
        ])
        .expect("export command should parse");

        let Command::Export(args) = cli.command else {
            panic!("expected export command");
        };

        assert_eq!(args.format, ExportFormat::Vtt);
    }
}
