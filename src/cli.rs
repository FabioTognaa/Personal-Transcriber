use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::domain::{ControlAction, InferenceConfig, SegmenterConfig};
use crate::model::ModelPreset;
use crate::session::StartOptions;
use crate::{Error, Result};

/// Fixed duration of the `doctor --probe-audio` check.
const DOCTOR_PROBE_DURATION: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Parser)]
#[command(
    name = "personal-transcriber",
    version,
    about = "Record and transcribe live meetings locally"
)]
pub struct Cli {
    /// Increase diagnostic verbosity. Repeat for more detail.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

/// Directory holding session directories. Only the commands that create or
/// address a session expose it, so `devices` and `transcribe` do not advertise a
/// flag they would ignore.
#[derive(Debug, Args)]
pub struct SessionsDirArgs {
    /// Directory under which session directories are created.
    #[arg(long, default_value = "sessions")]
    pub sessions_dir: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List audio devices visible to the application.
    Devices,
    /// Check local prerequisites without changing system configuration.
    Doctor(DoctorArgs),
    /// Start a local recording and transcription session.
    Start(StartArgs),
    /// Transcribe an existing audio file to `<stem>.txt` and `<stem>.md`.
    Transcribe(TranscribeArgs),
    /// Show the current session state and backlog.
    Status(SessionsDirArgs),
    /// Pause transcription while audio recording continues.
    Pause(SessionsDirArgs),
    /// Resume transcription for the active session.
    Resume(SessionsDirArgs),
    /// Finalize the active session.
    Stop(SessionsDirArgs),
    /// Export a completed canonical transcript.
    Export(ExportArgs),
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Local whisper.cpp GGML model path. Overrides `--model-preset`.
    #[arg(long)]
    pub model: Option<PathBuf>,

    /// Built-in model preset resolved under `models/`.
    #[arg(long, value_enum)]
    pub model_preset: Option<ModelPreset>,

    /// Open both live inputs and require signal on each.
    #[arg(long)]
    pub probe_audio: bool,

    /// Exact microphone input name used by the audio probe.
    #[arg(long, requires = "probe_audio")]
    pub microphone: Option<String>,

    /// Exact system input name used by the audio probe; defaults to BlackHole.
    #[arg(long, requires = "probe_audio")]
    pub system_audio: Option<String>,

    #[command(flatten)]
    pub sessions: SessionsDirArgs,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// Input device used for the local microphone.
    #[arg(long)]
    pub microphone: Option<String>,

    /// Input device used for remote/system audio, normally BlackHole on macOS.
    #[arg(long)]
    pub system_audio: Option<String>,

    #[command(flatten)]
    pub model: ModelArgs,

    #[command(flatten)]
    pub vad: VadArgs,

    #[command(flatten)]
    pub inference: InferenceArgs,

    #[command(flatten)]
    pub sessions: SessionsDirArgs,
}

#[derive(Debug, Args)]
pub struct TranscribeArgs {
    /// Audio file to transcribe (`.wav`, `.mp3`, `.m4a`, `.mp4`, `.aac`).
    pub input: PathBuf,

    #[command(flatten)]
    pub model: ModelArgs,

    #[command(flatten)]
    pub vad: VadArgs,

    #[command(flatten)]
    pub inference: InferenceArgs,
}

/// Model selection and language, shared by `start` and `transcribe`.
#[derive(Debug, Args)]
pub struct ModelArgs {
    /// Local whisper.cpp GGML model path. Overrides `--model-preset`.
    #[arg(long)]
    pub model: Option<PathBuf>,

    /// Built-in model preset resolved under `models/`; ignored when `--model` is set.
    /// Without either flag, the model is asked interactively on a terminal, otherwise
    /// the `small` preset is used.
    #[arg(long, value_enum)]
    pub model_preset: Option<ModelPreset>,

    /// Spoken language: `it` or `en`.
    #[arg(long, default_value = "it", value_parser = ["it", "en"])]
    pub language: String,

    /// Initial prompt that guides punctuation and vocabulary. Defaults to a
    /// built-in prompt for `--language`; pass an empty string to disable it.
    #[arg(long)]
    pub asr_prompt: Option<String>,
}

/// Voice-activity segmentation, shared by `start` and `transcribe`.
///
/// Only the parameters that realistically depend on the recording environment
/// are exposed; pre/post roll, minimum speech and maximum segment length keep
/// their defaults and are still recorded in `session.json`.
#[derive(Debug, Args)]
pub struct VadArgs {
    /// RMS threshold above which a PCM chunk is treated as voice.
    #[arg(long, default_value_t = 0.02)]
    pub vad_threshold: f32,

    /// Consecutive voice required to start a segment.
    #[arg(long, default_value_t = 60)]
    pub vad_start_ms: u64,

    /// Consecutive silence required to end a segment.
    #[arg(long, default_value_t = 800)]
    pub vad_end_ms: u64,
}

/// whisper.cpp decoding options, shared by `start` and `transcribe`.
///
/// Thread count, decoding strategy, beam width, temperature and Core ML keep
/// their defaults; flash attention stays a flag because it is the escape hatch
/// for Metal-specific issues.
#[derive(Debug, Args)]
pub struct InferenceArgs {
    /// Enable whisper.cpp flash attention.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub asr_flash_attention: bool,
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

    /// Permit exporting a transcript from a session that did not complete.
    #[arg(long)]
    pub allow_partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Jsonl,
    Text,
    Markdown,
    Srt,
    Vtt,
}

pub fn execute(command: Command) -> Result<()> {
    match command {
        Command::Devices => {
            print_json(&crate::diagnostics::audio_devices()?)?;
            Ok(())
        }
        Command::Doctor(args) => {
            let model = args.model.clone().unwrap_or_else(|| {
                args.model_preset
                    .unwrap_or(ModelPreset::Small)
                    .default_path()
            });
            let report = if args.probe_audio {
                crate::diagnostics::doctor_with_audio_probe(
                    &args.sessions.sessions_dir,
                    &model,
                    args.microphone.as_deref(),
                    args.system_audio.as_deref(),
                    DOCTOR_PROBE_DURATION,
                )
            } else {
                crate::diagnostics::doctor(&args.sessions.sessions_dir, &model)
            };
            print_json(&report)?;
            if report.ready {
                Ok(())
            } else {
                Err(Error::DoctorFailed)
            }
        }
        Command::Start(args) => {
            let model = resolve_model_path(args.model.model, args.model.model_preset)
                .or_else(select_model_interactively)
                .unwrap_or_else(|| ModelPreset::Small.default_path());
            let language = args.model.language;
            let prompt = resolve_initial_prompt(args.model.asr_prompt, &language);
            let root = crate::session::run(StartOptions {
                sessions_dir: args.sessions.sessions_dir,
                input_wav: None,
                model: Some(model),
                language,
                microphone: args.microphone,
                system_audio: args.system_audio,
                segmenter: segmenter_config(&args.vad),
                inference: inference_config(&args.inference, prompt),
                model_identity: None,
            })?;
            println!("{}", root.display());
            Ok(())
        }
        Command::Transcribe(args) => {
            let model = resolve_model_path(args.model.model, args.model.model_preset)
                .or_else(select_model_interactively)
                .unwrap_or_else(|| ModelPreset::Small.default_path());
            let language = args.model.language;
            let prompt = resolve_initial_prompt(args.model.asr_prompt, &language);
            crate::transcribe::run(crate::transcribe::TranscribeOptions {
                input: args.input,
                model,
                language,
                segmenter: segmenter_config(&args.vad),
                inference: inference_config(&args.inference, prompt),
            })?;
            Ok(())
        }
        Command::Status(args) => {
            print_json(&crate::control::status_report(&args.sessions_dir)?)?;
            Ok(())
        }
        Command::Pause(args) => {
            print_status(&crate::control::request(
                &args.sessions_dir,
                ControlAction::Pause,
            )?)?;
            Ok(())
        }
        Command::Resume(args) => {
            print_status(&crate::control::request(
                &args.sessions_dir,
                ControlAction::Resume,
            )?)?;
            Ok(())
        }
        Command::Stop(args) => {
            print_status(&crate::control::request(
                &args.sessions_dir,
                ControlAction::Stop,
            )?)?;
            Ok(())
        }
        Command::Export(args) => {
            crate::export::export_session(
                &args.session,
                args.format.into(),
                args.output.as_deref(),
                args.allow_partial,
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

/// Resolve the initial prompt: explicit override, otherwise the built-in
/// language default. An explicit empty string disables the prompt entirely.
fn resolve_initial_prompt(override_prompt: Option<String>, language: &str) -> Option<String> {
    match override_prompt {
        Some(prompt) if prompt.trim().is_empty() => None,
        Some(prompt) => Some(prompt),
        None => crate::asr::default_initial_prompt(language).map(str::to_owned),
    }
}

fn segmenter_config(vad: &VadArgs) -> SegmenterConfig {
    SegmenterConfig {
        energy_threshold: vad.vad_threshold,
        start_trigger_ms: vad.vad_start_ms,
        end_silence_ms: vad.vad_end_ms,
        ..SegmenterConfig::default()
    }
}

fn inference_config(inference: &InferenceArgs, prompt: Option<String>) -> InferenceConfig {
    InferenceConfig {
        flash_attention: inference.asr_flash_attention,
        prompt,
        ..InferenceConfig::default()
    }
}

/// Resolve the model from explicit arguments only (no interactive prompt).
/// `--model` wins over `--model-preset`; `None` means "ask or use the default".
fn resolve_model_path(model: Option<PathBuf>, preset: Option<ModelPreset>) -> Option<PathBuf> {
    model.or_else(|| preset.map(ModelPreset::default_path))
}

struct ModelOption {
    label: String,
    path: PathBuf,
    available: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum MenuChoice {
    Default,
    Select(usize),
    Custom,
}

/// Parse a menu answer: empty is the default, a valid number selects an option,
/// `p` asks for a custom path. Anything else falls back to the default.
fn parse_menu_selection(input: &str, option_count: usize) -> MenuChoice {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return MenuChoice::Default;
    }
    if trimmed.eq_ignore_ascii_case("p") {
        return MenuChoice::Custom;
    }
    match trimmed.parse::<usize>() {
        Ok(number) if (1..=option_count).contains(&number) => MenuChoice::Select(number - 1),
        _ => MenuChoice::Default,
    }
}

/// Presets first (always listed, marked available or not), then any other GGML
/// files found in `models_dir`.
fn model_menu_options(models_dir: &Path) -> Vec<ModelOption> {
    let mut options: Vec<ModelOption> = ModelPreset::ALL
        .iter()
        .map(|preset| {
            let path = preset.default_path();
            ModelOption {
                label: preset.label().to_owned(),
                available: path.is_file(),
                path,
            }
        })
        .collect();
    let known: Vec<PathBuf> = options.iter().map(|option| option.path.clone()).collect();
    let Ok(entries) = std::fs::read_dir(models_dir) else {
        return options;
    };
    let mut extra: Vec<ModelOption> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(std::ffi::OsStr::to_str) == Some("bin"))
        .filter(|path| !known.contains(path))
        .map(|path| ModelOption {
            label: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            available: true,
            path,
        })
        .collect();
    extra.sort_by(|left, right| left.label.cmp(&right.label));
    options.extend(extra);
    options
}

fn print_model_menu(options: &[ModelOption]) {
    println!("Seleziona il modello per la trascrizione:");
    for (index, option) in options.iter().enumerate() {
        let status = if option.available {
            "scaricato"
        } else {
            "mancante"
        };
        println!(
            "  {}) {:<16} [{}] {}",
            index + 1,
            option.label,
            status,
            option.path.display()
        );
    }
    print!("Invio = default (small), numero = scegli, p = percorso personalizzato: ");
    let _ = std::io::stdout().flush();
}

/// Interactive model picker. Shown only on an interactive terminal and only when
/// no model flag was given. Returns `None` to fall back to the `small` preset.
fn select_model_interactively() -> Option<PathBuf> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    let options = model_menu_options(Path::new(crate::model::MODELS_DIR));
    print_model_menu(&options);
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return None;
    }
    match parse_menu_selection(&input, options.len()) {
        MenuChoice::Default => None,
        MenuChoice::Select(index) => options.get(index).map(|option| option.path.clone()),
        MenuChoice::Custom => {
            print!("Percorso del modello (.bin): ");
            let _ = std::io::stdout().flush();
            let mut path = String::new();
            if std::io::stdin().read_line(&mut path).is_err() {
                return None;
            }
            let path = path.trim();
            if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(path))
            }
        }
    }
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
            "personal-transcriber",
            "start",
            "--model",
            "models/fixture.bin",
        ])
        .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.language, "it");
        assert_eq!(args.model.model, Some(PathBuf::from("models/fixture.bin")));
        assert_eq!(args.sessions.sessions_dir, PathBuf::from("sessions"));
    }

    #[test]
    fn start_defaults_to_the_small_model_preset() {
        let cli = Cli::try_parse_from(["personal-transcriber", "start"])
            .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.model, None);
        assert_eq!(args.model.model_preset, None);
        assert_eq!(
            resolve_model_path(args.model.model, args.model.model_preset)
                .unwrap_or_else(|| ModelPreset::Small.default_path()),
            PathBuf::from("models/ggml-small-q5_1.bin")
        );
    }

    #[test]
    fn start_accepts_a_model_preset() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "start",
            "--model-preset",
            "large-v3-turbo",
        ])
        .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.model_preset, Some(ModelPreset::LargeV3Turbo));
        assert_eq!(
            args.model.model_preset.unwrap().default_path(),
            PathBuf::from("models/ggml-large-v3-turbo-q8_0.bin")
        );
    }

    #[test]
    fn explicit_model_path_overrides_the_preset() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "start",
            "--model-preset",
            "large-v3",
            "--model",
            "models/custom.bin",
        ])
        .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(
            resolve_model_path(args.model.model, args.model.model_preset),
            Some(PathBuf::from("models/custom.bin"))
        );
    }

    #[test]
    fn model_resolution_uses_the_preset_or_nothing() {
        assert_eq!(
            resolve_model_path(None, Some(ModelPreset::LargeV3Turbo)),
            Some(PathBuf::from("models/ggml-large-v3-turbo-q8_0.bin"))
        );
        assert_eq!(resolve_model_path(None, None), None);
    }

    #[test]
    fn menu_selection_parsing() {
        assert_eq!(parse_menu_selection("", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("  2 ", 4), MenuChoice::Select(1));
        assert_eq!(parse_menu_selection("5", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("0", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("p", 4), MenuChoice::Custom);
        assert_eq!(parse_menu_selection("abc", 4), MenuChoice::Default);
    }

    #[test]
    fn menu_lists_presets_then_extra_ggml_files() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        std::fs::write(temporary.path().join("custom.bin"), b"x")
            .expect("fixture should be written");
        std::fs::write(temporary.path().join("notes.txt"), b"x")
            .expect("fixture should be written");

        let options = model_menu_options(temporary.path());

        assert_eq!(options.len(), ModelPreset::ALL.len() + 1);
        assert!(
            options
                .iter()
                .any(|option| option.label == "custom.bin" && option.available)
        );
        assert!(!options.iter().any(|option| option.label == "notes.txt"));
    }

    #[test]
    fn default_prompt_matches_the_language_and_can_be_disabled() {
        assert!(resolve_initial_prompt(None, "it").is_some());
        assert!(resolve_initial_prompt(None, "en").is_some());
        assert_eq!(resolve_initial_prompt(None, "fr"), None);
        assert_eq!(resolve_initial_prompt(Some(String::new()), "it"), None);
        assert_eq!(
            resolve_initial_prompt(Some("glossario".to_owned()), "it"),
            Some("glossario".to_owned())
        );
    }

    #[test]
    fn start_accepts_english() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "start",
            "--model",
            "models/fixture.bin",
            "--language",
            "en",
        ])
        .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.language, "en");
    }

    #[test]
    fn export_format_parses_from_cli() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
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
