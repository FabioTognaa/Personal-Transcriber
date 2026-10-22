use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
/// address a session expose it, so `devices` does not advertise a flag it would
/// ignore.
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
    /// Transcribe an existing audio file into a new directory under `sessions/`.
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

    /// Minutes of consecutive silence after which the session closes itself.
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..=30))]
    pub silence_timeout: u64,

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
    /// Audio file to transcribe (`.wav`, `.mp3`, `.m4a`, `.mp4`, `.aac`). Asked
    /// interactively on a terminal when omitted.
    pub input: Option<PathBuf>,

    #[command(flatten)]
    pub model: ModelArgs,

    #[command(flatten)]
    pub vad: VadArgs,

    #[command(flatten)]
    pub inference: InferenceArgs,

    #[command(flatten)]
    pub sessions: SessionsDirArgs,
}

/// Model selection and language, shared by `start` and `transcribe`.
#[derive(Debug, Args)]
pub struct ModelArgs {
    /// Local whisper.cpp GGML model path. Overrides `--model-preset`.
    #[arg(long)]
    pub model: Option<PathBuf>,

    /// Built-in model preset resolved under `models/`; ignored when `--model` is set.
    /// Without either flag, the model is asked interactively on a terminal, otherwise
    /// the `large-v3-turbo` preset is used.
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

/// Voice-activity segmentation, shared by `start` and `transcribe`. All
/// `SegmenterConfig` parameters are exposed; the defaults are tuned for
/// continuous meeting speech and everything is recorded in `session.json`.
#[derive(Debug, Args)]
pub struct VadArgs {
    /// Minimum RMS threshold for voice; the adaptive noise-based threshold
    /// never drops below it.
    #[arg(long, default_value_t = 0.02)]
    pub vad_threshold: f32,

    /// Voice must exceed the estimated background noise by this factor to
    /// count as speech.
    #[arg(long, default_value_t = crate::domain::default_noise_multiplier())]
    pub vad_noise_multiplier: f32,

    /// Consecutive voice required to start a segment.
    #[arg(long, default_value_t = 60)]
    pub vad_start_ms: u64,

    /// Consecutive silence required to end a segment.
    #[arg(long, default_value_t = 1600)]
    pub vad_end_ms: u64,

    /// Audio kept before the first voice chunk.
    #[arg(long, default_value_t = 200)]
    pub vad_pre_roll_ms: u64,

    /// Audio kept after the last voice chunk.
    #[arg(long, default_value_t = 200)]
    pub vad_post_roll_ms: u64,

    /// Speech shorter than this is discarded as noise (counted in status.json).
    #[arg(long, default_value_t = 100)]
    pub vad_min_speech_ms: u64,

    /// Ceiling for a segment; longer speech is split on its longest pause.
    #[arg(long, default_value_t = 30_000)]
    pub vad_max_segment_ms: u64,
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

    /// Carry the tail of the previous segment's text into the initial prompt
    /// (opt-in: a repeated early mistake can leak into later segments).
    #[arg(long, default_value_t = false)]
    pub asr_context: bool,
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
            let model = doctor_model_path(args.model.clone(), args.model_preset);
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
            let session_name = match select_session_name_interactively(&args.sessions.sessions_dir)
            {
                NamePick::Use(name) => Some(name),
                NamePick::Skipped => None,
                NamePick::Cancelled => return Ok(()),
            };
            let Some(model) = resolve_model(args.model.model, args.model.model_preset)? else {
                return Ok(());
            };
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
                session_name,
                silence_timeout: Some(Duration::from_secs(args.silence_timeout * 60)),
            })?;
            println!("{}", root.display());
            Ok(())
        }
        Command::Transcribe(args) => {
            let session_name = match select_session_name_interactively(&args.sessions.sessions_dir)
            {
                NamePick::Use(name) => Some(name),
                NamePick::Skipped => None,
                NamePick::Cancelled => return Ok(()),
            };
            let input = match args.input {
                Some(input) => input,
                None => match select_input_path_interactively()? {
                    InputPick::Use(path) => path,
                    InputPick::Cancelled => return Ok(()),
                },
            };
            let Some(model) = resolve_model(args.model.model, args.model.model_preset)? else {
                return Ok(());
            };
            let language = args.model.language;
            let prompt = resolve_initial_prompt(args.model.asr_prompt, &language);
            crate::transcribe::run(crate::transcribe::TranscribeOptions {
                input,
                model,
                sessions_dir: args.sessions.sessions_dir,
                language,
                segmenter: segmenter_config(&args.vad),
                inference: inference_config(&args.inference, prompt),
                session_name,
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
        noise_multiplier: vad.vad_noise_multiplier,
        start_trigger_ms: vad.vad_start_ms,
        end_silence_ms: vad.vad_end_ms,
        pre_roll_ms: vad.vad_pre_roll_ms,
        post_roll_ms: vad.vad_post_roll_ms,
        min_speech_ms: vad.vad_min_speech_ms,
        max_segment_ms: vad.vad_max_segment_ms,
    }
}

fn inference_config(inference: &InferenceArgs, prompt: Option<String>) -> InferenceConfig {
    InferenceConfig {
        flash_attention: inference.asr_flash_attention,
        context_enabled: inference.asr_context,
        prompt,
        ..InferenceConfig::default()
    }
}

/// Resolve the model from explicit arguments only (no interactive prompt).
/// `--model` wins over `--model-preset`; `None` means "ask or use the default".
fn resolve_model_path(model: Option<PathBuf>, preset: Option<ModelPreset>) -> Option<PathBuf> {
    model.or_else(|| preset.map(ModelPreset::default_path))
}

/// Model `doctor` verifies when no flag is given: the same default `start` and
/// `transcribe` resolve, so a green `doctor` checks the model you will run.
fn doctor_model_path(model: Option<PathBuf>, preset: Option<ModelPreset>) -> PathBuf {
    model.unwrap_or_else(|| preset.unwrap_or(ModelPreset::LargeV3Turbo).default_path())
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
    Cancel,
}

/// Parse a menu answer: empty is the default, a valid number selects an option,
/// `p` asks for a custom path, `q` cancels. Anything else falls back to the default.
fn parse_menu_selection(input: &str, option_count: usize) -> MenuChoice {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return MenuChoice::Default;
    }
    if trimmed.eq_ignore_ascii_case("p") {
        return MenuChoice::Custom;
    }
    if trimmed.eq_ignore_ascii_case("q") {
        return MenuChoice::Cancel;
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
    print!(
        "Invio = default (large-v3-turbo), numero = scegli, p = percorso personalizzato, q = annulla: "
    );
    let _ = std::io::stdout().flush();
}

/// Outcome of the interactive picker: a model to use, or the user cancelled.
enum ModelPick {
    Use(PathBuf),
    Cancelled,
}

fn default_model() -> ModelPick {
    ModelPick::Use(default_model_path())
}

/// Interactive model picker. Shown only on an interactive terminal and only when
/// no model flag was given. Returns the `large-v3-turbo` preset on an empty answer.
fn select_model_interactively() -> ModelPick {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return default_model();
    }
    let options = model_menu_options(Path::new(crate::model::MODELS_DIR));
    print_model_menu(&options);
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return default_model();
    }
    match parse_menu_selection(&input, options.len()) {
        MenuChoice::Default => default_model(),
        MenuChoice::Select(index) => ModelPick::Use(
            options
                .get(index)
                .map(|option| option.path.clone())
                .unwrap_or_else(default_model_path),
        ),
        MenuChoice::Custom => {
            print!("Percorso del modello (.bin): ");
            let _ = std::io::stdout().flush();
            let mut path = String::new();
            if std::io::stdin().read_line(&mut path).is_err() {
                return default_model();
            }
            let path = path.trim();
            if path.is_empty() {
                default_model()
            } else {
                ModelPick::Use(PathBuf::from(path))
            }
        }
        MenuChoice::Cancel => {
            println!("selezione annullata");
            ModelPick::Cancelled
        }
    }
}

fn default_model_path() -> PathBuf {
    ModelPreset::LargeV3Turbo.default_path()
}

/// Characters that make a session name unusable on any supported platform; the
/// user is asked again. Space is allowed and normalized to `_` before this check.
const SESSION_NAME_INVALID_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Normalize a typed session name (trim, spaces to `_`) and reject an empty
/// result or one containing a forbidden character.
fn clean_session_name(input: &str) -> std::result::Result<String, &'static str> {
    let name = input.trim().replace(' ', "_");
    if name.is_empty() {
        return Err("inserisci un nome");
    }
    if name.contains(SESSION_NAME_INVALID_CHARS) {
        return Err("i caratteri / \\ : * ? \" < > | non sono consentiti");
    }
    Ok(name)
}

/// Where `start` would place the session for `name` right now, for the
/// "already exists" check. `None` when the clock cannot be read.
fn candidate_session_directory(sessions_dir: &Path, name: &str) -> Option<PathBuf> {
    let directory_name =
        crate::storage::session_directory_name(crate::storage::unix_time_ms().ok()?, Some(name))
            .ok()?;
    Some(sessions_dir.join(directory_name))
}

/// Outcome of the interactive session-name prompt: a usable name, or skipped
/// on a non-interactive terminal, or the user cancelled.
enum NamePick {
    Use(String),
    Skipped,
    Cancelled,
}

/// Ask for the recording's repository name, used to build the
/// `YY-MM-DD_<name>` session directory. Asked only on an interactive terminal so
/// non-interactive runs keep the date-only name. `q` cancels, like the model menu.
fn select_session_name_interactively(sessions_dir: &Path) -> NamePick {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return NamePick::Skipped;
    }
    loop {
        print!("Nome della repo per la registrazione (q = annulla): ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return NamePick::Skipped;
        }
        if input.trim().eq_ignore_ascii_case("q") {
            println!("selezione annullata");
            return NamePick::Cancelled;
        }
        let name = match clean_session_name(&input) {
            Ok(name) => name,
            Err(reason) => {
                println!("nome non valido: {reason}");
                continue;
            }
        };
        if let Some(directory) = candidate_session_directory(sessions_dir, &name)
            && directory.exists()
        {
            println!(
                "esiste già una sessione \"{}\": scegli un altro nome",
                directory.file_name().unwrap_or_default().to_string_lossy()
            );
            continue;
        }
        return NamePick::Use(name);
    }
}

/// Outcome of the interactive input-path prompt.
enum InputPick {
    Use(PathBuf),
    Cancelled,
}

/// Ask for the audio file to transcribe. Only reached when no path was passed
/// on the command line, so a non-interactive run has no way to answer: it fails
/// asking for the argument instead of silently hanging.
fn select_input_path_interactively() -> Result<InputPick> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::InputRequired);
    }
    loop {
        print!("Percorso del file audio (q = annulla): ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return Err(Error::InputRequired);
        }
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("q") {
            println!("selezione annullata");
            return Ok(InputPick::Cancelled);
        }
        if trimmed.is_empty() {
            println!("percorso non valido: inserisci un percorso");
            continue;
        }
        let path = PathBuf::from(trimmed);
        if !path.is_file() {
            println!("file non trovato: {}", path.display());
            continue;
        }
        return Ok(InputPick::Use(path));
    }
}

/// Resolve the ASR model: the `--model` flag or preset, otherwise the interactive
/// picker. `Ok(None)` means the user cancelled the picker; the command exits cleanly.
fn resolve_model(model: Option<PathBuf>, preset: Option<ModelPreset>) -> Result<Option<PathBuf>> {
    if let Some(path) = resolve_model_path(model, preset) {
        return Ok(Some(path));
    }
    Ok(match select_model_interactively() {
        ModelPick::Use(path) => Some(path),
        ModelPick::Cancelled => None,
    })
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
    fn start_defaults_to_the_large_v3_turbo_model_preset() {
        let cli = Cli::try_parse_from(["personal-transcriber", "start"])
            .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.model, None);
        assert_eq!(args.model.model_preset, None);
        assert_eq!(
            resolve_model_path(args.model.model, args.model.model_preset)
                .unwrap_or_else(default_model_path),
            PathBuf::from("models/ggml-large-v3-turbo-q8_0.bin")
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
    fn doctor_defaults_to_the_same_model_as_start() {
        // `doctor` must verify the model `start`/`transcribe` will actually use,
        // so a green check is meaningful when no model flag is passed.
        assert_eq!(
            doctor_model_path(None, None),
            resolve_model_path(None, None).unwrap_or_else(default_model_path)
        );
        assert_eq!(
            doctor_model_path(None, None),
            PathBuf::from("models/ggml-large-v3-turbo-q8_0.bin")
        );
    }

    #[test]
    fn session_name_normalizes_spaces_and_rejects_invalid_characters() {
        assert_eq!(clean_session_name("  my repo  ").as_deref(), Ok("my_repo"));
        assert!(clean_session_name("/etc/passwd").is_err());
        assert!(clean_session_name("weird:name*").is_err());
        assert!(clean_session_name("back\\slash").is_err());
        assert!(clean_session_name("   ").is_err());
    }

    #[test]
    fn menu_selection_parsing() {
        assert_eq!(parse_menu_selection("", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("  2 ", 4), MenuChoice::Select(1));
        assert_eq!(parse_menu_selection("5", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("0", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("p", 4), MenuChoice::Custom);
        assert_eq!(parse_menu_selection("abc", 4), MenuChoice::Default);
        assert_eq!(parse_menu_selection("q", 4), MenuChoice::Cancel);
        assert_eq!(parse_menu_selection(" Q ", 4), MenuChoice::Cancel);
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
    fn silence_timeout_defaults_to_fifteen_minutes_and_only_accepts_one_to_thirty() {
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
        assert_eq!(args.silence_timeout, 15);

        let cli = Cli::try_parse_from(["personal-transcriber", "start", "--silence-timeout", "30"])
            .expect("30 minutes should be accepted");
        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };
        assert_eq!(args.silence_timeout, 30);

        for value in ["0", "31", "-1", "1.5", "abc"] {
            assert!(
                Cli::try_parse_from(["personal-transcriber", "start", "--silence-timeout", value])
                    .is_err(),
                "{value} must be rejected"
            );
        }
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

    #[test]
    fn transcribe_input_path_is_optional() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "transcribe",
            "input.mp3",
            "--model",
            "models/fixture.bin",
        ])
        .expect("transcribe with a path should parse");
        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };
        assert_eq!(args.input, Some(PathBuf::from("input.mp3")));

        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "transcribe",
            "--model",
            "models/fixture.bin",
        ])
        .expect("transcribe without a path should parse");
        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };
        assert_eq!(args.input, None);
    }

    #[test]
    fn vad_and_context_flags_wire_into_their_configs() {
        let cli = Cli::try_parse_from([
            "personal-transcriber",
            "transcribe",
            "input.mp3",
            "--model",
            "models/fixture.bin",
            "--vad-end-ms",
            "2400",
            "--vad-pre-roll-ms",
            "300",
            "--vad-post-roll-ms",
            "250",
            "--vad-min-speech-ms",
            "120",
            "--vad-max-segment-ms",
            "25000",
            "--vad-noise-multiplier",
            "5.0",
            "--asr-context",
        ])
        .expect("transcribe command should parse");

        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };

        assert_eq!(
            segmenter_config(&args.vad),
            SegmenterConfig {
                energy_threshold: 0.02,
                noise_multiplier: 5.0,
                start_trigger_ms: 60,
                end_silence_ms: 2400,
                pre_roll_ms: 300,
                post_roll_ms: 250,
                min_speech_ms: 120,
                max_segment_ms: 25000,
            }
        );
        assert!(inference_config(&args.inference, None).context_enabled);
    }
}
