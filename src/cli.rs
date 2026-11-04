use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::domain::{ControlAction, InferenceConfig, SegmenterConfig};
use crate::model::{ArtifactKind, ModelArtifact, ModelPreset};
use crate::session::StartOptions;
use crate::{Error, Result};

/// Fixed duration of the `doctor --probe-audio` check.
const DOCTOR_PROBE_DURATION: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Parser)]
#[command(
    name = "scrib",
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

    /// Built-in model preset resolved under the models directory.
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

    /// Built-in model preset resolved under the models directory; ignored when `--model` is set.
    /// Without either flag, the model is asked interactively on a terminal; a
    /// non-interactive run without a model fails.
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
            let model = args
                .model
                .clone()
                .or_else(|| args.model_preset.map(ModelPreset::default_path));
            let Some(model) = model else {
                return Err(Error::ModelRequired);
            };
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
            let prompt = resolve_initial_prompt_for_model(&model, args.model.asr_prompt, &language);
            let root = crate::session::run(StartOptions {
                sessions_dir: args.sessions.sessions_dir,
                input_wav: None,
                model: Some(model),
                language,
                microphone: args.microphone,
                system_audio: args.system_audio,
                segmenter: segmenter_config(&args.vad),
                inference: inference_config(
                    &args.inference,
                    prompt,
                    crate::domain::default_temperature_inc(),
                ),
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
            let prompt = resolve_initial_prompt_for_model(&model, args.model.asr_prompt, &language);
            crate::transcribe::run(crate::transcribe::TranscribeOptions {
                input,
                model,
                sessions_dir: args.sessions.sessions_dir,
                language,
                inference: inference_config(&args.inference, prompt, 0.0),
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

/// The built-in prompt is Whisper-only. A Parakeet bundle keeps only an explicit
/// `--asr-prompt` (which the engine warns it ignores), so a normal Parakeet run
/// does not receive a prompt it cannot use.
fn resolve_initial_prompt_for_model(
    model: &Path,
    override_prompt: Option<String>,
    language: &str,
) -> Option<String> {
    if ModelArtifact::detect(model).is_ok_and(|artifact| artifact.kind == ArtifactKind::Bundle) {
        return override_prompt.filter(|prompt| !prompt.trim().is_empty());
    }
    resolve_initial_prompt(override_prompt, language)
}

fn segmenter_config(vad: &VadArgs) -> SegmenterConfig {
    SegmenterConfig {
        energy_threshold: vad.vad_threshold,
        start_trigger_ms: vad.vad_start_ms,
        end_silence_ms: vad.vad_end_ms,
        ..SegmenterConfig::default()
    }
}

fn inference_config(
    inference: &InferenceArgs,
    prompt: Option<String>,
    temperature_inc: f32,
) -> InferenceConfig {
    InferenceConfig {
        flash_attention: inference.asr_flash_attention,
        prompt,
        temperature_inc,
        ..InferenceConfig::default()
    }
}

struct ModelOption {
    label: String,
    path: PathBuf,
    size_bytes: u64,
}

/// Only artifacts present on disk are listed, smallest first. Presets are not
/// advertised when their file or directory is missing.
fn model_menu_options(models_dir: &Path) -> Vec<ModelOption> {
    let Ok(entries) = std::fs::read_dir(models_dir) else {
        return Vec::new();
    };
    let mut options: Vec<ModelOption> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        })
        .filter_map(|path| {
            // An explicit `--model` may be any file, but the menu only offers
            // Whisper `.bin` files and directory bundles.
            if path.is_file() && path.extension().and_then(std::ffi::OsStr::to_str) != Some("bin") {
                return None;
            }
            let artifact = ModelArtifact::detect(&path).ok()?;
            // A bundle needs the ONNX Runtime engine. Without the feature the
            // binary cannot run it, so the menu must not offer it.
            if artifact.kind == ArtifactKind::Bundle && !cfg!(feature = "parakeet") {
                return None;
            }
            let size_bytes = artifact.total_size().unwrap_or(0);
            let label = ModelPreset::ALL
                .iter()
                .find(|preset| preset.default_path() == path)
                .map_or_else(|| artifact.name(), |preset| preset.label().to_owned());
            Some(ModelOption {
                label,
                path,
                size_bytes,
            })
        })
        .collect();
    options.sort_by(|left, right| {
        left.size_bytes
            .cmp(&right.size_bytes)
            .then_with(|| left.label.cmp(&right.label))
    });
    options
}

fn human_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

/// Interactive model picker. `Ok(None)` means the user cancelled.
fn select_model_interactively() -> Result<Option<PathBuf>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::ModelRequired);
    }
    let models_dir = crate::model::models_dir();
    let options = model_menu_options(&models_dir);
    if options.is_empty() {
        return Err(Error::NoModelsAvailable(models_dir));
    }
    if options.len() == 1 {
        let option = &options[0];
        println!(
            "Modello selezionato: {} ({})",
            option.label,
            option.path.display()
        );
        return Ok(Some(option.path.clone()));
    }

    let custom = options.len();
    let mut menu = cliclack::select("Seleziona il modello per la trascrizione");
    for (index, option) in options.iter().enumerate() {
        menu = menu.item(
            index,
            format!("{}  {}", option.label, human_size(option.size_bytes)),
            option.path.display(),
        );
    }
    let choice = menu
        .item(
            custom,
            "Percorso personalizzato…",
            "file .bin o directory ONNX",
        )
        .interact();
    let index = match choice {
        Ok(index) => index,
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if index != custom {
        return Ok(Some(options[index].path.clone()));
    }

    let mut path_input = cliclack::input("Percorso del modello")
        .placeholder("file .bin o directory ONNX")
        .validate(|path: &String| {
            if path.trim().is_empty() {
                return Err("inserisci un percorso".to_owned());
            }
            ModelArtifact::detect(Path::new(path.trim()))
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
    match path_input.interact::<String>() {
        Ok(path) => Ok(Some(PathBuf::from(path.trim()))),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(None),
        Err(error) => Err(error.into()),
    }
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
/// non-interactive runs keep the date-only name. Esc or Ctrl-C cancels, like the
/// model menu.
fn select_session_name_interactively(sessions_dir: &Path) -> NamePick {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return NamePick::Skipped;
    }
    let sessions_dir = sessions_dir.to_path_buf();
    let mut prompt = cliclack::input("Nome della repo per la registrazione")
        .placeholder("es. meeting_notes")
        .validate(move |input: &String| {
            let name = clean_session_name(input).map_err(str::to_owned)?;
            if let Some(directory) = candidate_session_directory(&sessions_dir, &name)
                && directory.exists()
            {
                return Err(format!(
                    "esiste già una sessione \"{}\": scegli un altro nome",
                    directory.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            Ok(())
        });
    match prompt.interact::<String>() {
        // The validator has already accepted the name; clean it once more to
        // return the normalized form.
        Ok(name) => match clean_session_name(&name) {
            Ok(name) => NamePick::Use(name),
            Err(_) => NamePick::Skipped,
        },
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => NamePick::Cancelled,
        Err(_) => NamePick::Skipped,
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

/// Resolve the ASR model: the `--model` flag, a preset that is present on disk,
/// otherwise the interactive picker. `Ok(None)` means the user cancelled.
fn resolve_model(model: Option<PathBuf>, preset: Option<ModelPreset>) -> Result<Option<PathBuf>> {
    if let Some(path) = model {
        return Ok(Some(path));
    }
    if let Some(preset) = preset {
        let path = preset.default_path();
        if !path.exists() {
            return Err(Error::ModelNotDownloaded {
                name: preset.label().to_owned(),
                path,
            });
        }
        return Ok(Some(path));
    }
    select_model_interactively()
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
        let cli = Cli::try_parse_from(["scrib", "start", "--model", "models/fixture.bin"])
            .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.language, "it");
        assert_eq!(args.model.model, Some(PathBuf::from("models/fixture.bin")));
        assert_eq!(args.sessions.sessions_dir, PathBuf::from("sessions"));
    }

    #[test]
    fn start_has_no_default_model() {
        let cli = Cli::try_parse_from(["scrib", "start"]).expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.model, None);
        assert_eq!(args.model.model_preset, None);
    }

    #[test]
    fn start_accepts_a_model_preset() {
        let cli = Cli::try_parse_from(["scrib", "start", "--model-preset", "parakeet-v3"])
            .expect("start command should parse");

        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };

        assert_eq!(args.model.model_preset, Some(ModelPreset::ParakeetV3));
        assert_eq!(
            args.model.model_preset.unwrap().default_path(),
            crate::model::models_dir().join("parakeet-tdt-0.6b-v3-onnx")
        );
    }

    #[test]
    fn explicit_model_path_overrides_the_preset() {
        let cli = Cli::try_parse_from([
            "scrib",
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
            resolve_model(args.model.model, args.model.model_preset).expect("explicit path"),
            Some(PathBuf::from("models/custom.bin"))
        );
    }

    #[test]
    fn menu_lists_only_available_artifacts_sorted_by_size() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        std::fs::write(temporary.path().join("custom.bin"), b"x")
            .expect("fixture should be written");
        std::fs::write(temporary.path().join("notes.txt"), b"x")
            .expect("fixture should be written");
        let bundle = temporary.path().join("bundle");
        std::fs::create_dir(&bundle).expect("bundle directory should be created");
        for name in [
            crate::model::PARAKEET_CONFIG,
            crate::model::PARAKEET_VOCAB,
            "nemo128.onnx",
            crate::model::PARAKEET_ENCODER,
            crate::model::PARAKEET_DECODER_JOINT,
        ] {
            std::fs::write(bundle.join(name), b"x").expect("fixture should be written");
        }

        let options = model_menu_options(temporary.path());

        assert!(!options.iter().any(|option| option.label == "notes.txt"));
        #[cfg(feature = "parakeet")]
        {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].label, "custom.bin");
            assert_eq!(options[0].size_bytes, 1);
            assert_eq!(options[1].label, "bundle");
            assert_eq!(options[1].size_bytes, 5);
        }
        #[cfg(not(feature = "parakeet"))]
        {
            // Without the feature the binary cannot run a bundle, so it is hidden.
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].label, "custom.bin");
        }
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
            "scrib",
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
        let cli = Cli::try_parse_from(["scrib", "export", "sessions/example", "--format", "vtt"])
            .expect("export command should parse");

        let Command::Export(args) = cli.command else {
            panic!("expected export command");
        };

        assert_eq!(args.format, ExportFormat::Vtt);
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
    fn transcribe_input_path_is_optional() {
        let cli = Cli::try_parse_from([
            "scrib",
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

        let cli = Cli::try_parse_from(["scrib", "transcribe", "--model", "models/fixture.bin"])
            .expect("transcribe without a path should parse");
        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };
        assert_eq!(args.input, None);
    }

    #[test]
    fn transcribe_rejects_vad_flags_that_only_start_accepts() {
        assert!(
            Cli::try_parse_from([
                "scrib",
                "transcribe",
                "--model",
                "models/fixture.bin",
                "--vad-end-ms",
                "800",
            ])
            .is_err(),
            "transcribe no longer uses the live VAD"
        );
        assert!(
            Cli::try_parse_from([
                "scrib",
                "start",
                "--model",
                "models/fixture.bin",
                "--vad-end-ms",
                "800",
            ])
            .is_ok(),
            "start keeps the live VAD flags"
        );
    }

    #[test]
    fn silence_timeout_defaults_to_fifteen_minutes_and_only_accepts_one_to_thirty() {
        let cli = Cli::try_parse_from(["scrib", "start", "--model", "models/fixture.bin"])
            .expect("start command should parse");
        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };
        assert_eq!(args.silence_timeout, 15);

        let cli = Cli::try_parse_from(["scrib", "start", "--silence-timeout", "30"])
            .expect("30 minutes should be accepted");
        let Command::Start(args) = cli.command else {
            panic!("expected start command");
        };
        assert_eq!(args.silence_timeout, 30);

        for value in ["0", "31", "-1", "1.5", "abc"] {
            assert!(
                Cli::try_parse_from(["scrib", "start", "--silence-timeout", value]).is_err(),
                "{value} must be rejected"
            );
        }
    }
}
