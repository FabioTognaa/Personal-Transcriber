use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use personal_transcriber::asr::{AsrEngine, WhisperEngine, model_identity, sha256_file};
use personal_transcriber::audio::FileAudioSource;
use personal_transcriber::domain::{
    InferenceConfig, InferenceStrategy, SpeechSegment, TARGET_SAMPLE_RATE_HZ, TimestampUs,
    default_threads,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(about = "Reproducible local whisper.cpp benchmark")]
struct Args {
    #[arg(long)]
    model: PathBuf,
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    reference: Option<PathBuf>,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value = "it", value_parser = ["it", "en"])]
    language: String,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long, default_value_t = 1)]
    warmup: usize,
    #[arg(long, default_value_t = 5)]
    iterations: usize,
    #[arg(long, default_value_t = default_threads())]
    threads: i32,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    flash_attention: bool,
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    coreml: bool,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    generated_at_unix_ms: u128,
    git_commit: String,
    rust_version: String,
    whisper_rs_version: &'static str,
    whisper_cpp_version: &'static str,
    whisper_system_info: String,
    hardware: Hardware,
    model: Model,
    input: Input,
    inference: InferenceConfig,
    model_load_us: u64,
    warmup_iterations: usize,
    runs: Vec<Run>,
    median_rtf: f64,
    p95_rtf: f64,
    peak_rss_bytes: Option<u64>,
    quality: Option<Quality>,
}

#[derive(Debug, Serialize)]
struct Hardware {
    os_version: String,
    chip: String,
    memory_bytes: String,
}

#[derive(Debug, Serialize)]
struct Model {
    path: PathBuf,
    name: String,
    sha256: String,
    size_bytes: u64,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct Input {
    path: PathBuf,
    sha256: String,
    duration_us: u64,
    sample_rate_hz: u32,
    channels: u16,
}

#[derive(Debug, Serialize)]
struct Run {
    iteration: usize,
    inference_us: u64,
    rtf: f64,
    text: String,
}

#[derive(Debug, Serialize)]
struct Quality {
    reference_path: PathBuf,
    reference_sha256: String,
    wer: f64,
    cer: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.iterations == 0 {
        return Err("--iterations must be greater than zero".into());
    }
    let config = InferenceConfig {
        threads: args.threads,
        strategy: InferenceStrategy::BeamSearch,
        best_of: 5,
        beam_size: 5,
        temperature: 0.0,
        flash_attention: args.flash_attention,
        coreml: args.coreml,
        prompt: args.prompt.clone(),
    };
    let identity = model_identity(&args.model)?;
    let source = FileAudioSource::open(&args.input)?;
    let samples = source
        .chunks()
        .flat_map(|chunk| chunk.samples)
        .collect::<Vec<_>>();
    let duration_us = samples.len() as u64 * 1_000_000 / u64::from(TARGET_SAMPLE_RATE_HZ);
    let segment = SpeechSegment {
        start: TimestampUs(0),
        end: TimestampUs(duration_us),
        samples,
    };

    let load_started = Instant::now();
    let mut engine = WhisperEngine::load(&args.model, &config)?;
    let model_load_us = elapsed_us(load_started);
    for _ in 0..args.warmup {
        engine.transcribe(&segment, &args.language, &config)?;
    }

    let mut runs = Vec::with_capacity(args.iterations);
    for iteration in 0..args.iterations {
        let output = engine.transcribe(&segment, &args.language, &config)?;
        runs.push(Run {
            iteration,
            inference_us: output.inference_duration_us,
            rtf: output.inference_duration_us as f64 / duration_us as f64,
            text: output.text,
        });
    }
    let mut rtfs = runs.iter().map(|run| run.rtf).collect::<Vec<_>>();
    rtfs.sort_by(f64::total_cmp);
    let median_rtf = percentile(&rtfs, 0.5);
    let p95_rtf = percentile(&rtfs, 0.95);

    let quality = args
        .reference
        .as_ref()
        .map(|path| {
            let reference = fs::read_to_string(path)?;
            let hypothesis = &runs.last().expect("at least one measured iteration").text;
            Ok::<_, Box<dyn std::error::Error>>(Quality {
                reference_path: path.clone(),
                reference_sha256: sha256_file(path)?,
                wer: error_rate(
                    &normalize(&reference).split_whitespace().collect::<Vec<_>>(),
                    &normalize(hypothesis).split_whitespace().collect::<Vec<_>>(),
                ),
                cer: error_rate(
                    &normalize(&reference).chars().collect::<Vec<_>>(),
                    &normalize(hypothesis).chars().collect::<Vec<_>>(),
                ),
            })
        })
        .transpose()?;

    let report = BenchmarkReport {
        generated_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        git_commit: command_output("git", &["rev-parse", "HEAD"]),
        rust_version: command_output("rustc", &["--version"]),
        whisper_rs_version: "0.16.0",
        whisper_cpp_version: whisper_rs::WHISPER_CPP_VERSION,
        whisper_system_info: whisper_rs::print_system_info().to_owned(),
        hardware: Hardware {
            os_version: command_output("sw_vers", &["-productVersion"]),
            chip: command_output("sysctl", &["-n", "machdep.cpu.brand_string"]),
            memory_bytes: command_output("sysctl", &["-n", "hw.memsize"]),
        },
        model: Model {
            path: identity.path,
            name: identity.name,
            sha256: identity.sha256,
            size_bytes: identity.size_bytes,
            source: "https://huggingface.co/ggerganov/whisper.cpp",
        },
        input: Input {
            path: args.input.clone(),
            sha256: sha256_file(&args.input)?,
            duration_us,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: 1,
        },
        inference: config,
        model_load_us,
        warmup_iterations: args.warmup,
        runs,
        median_rtf,
        p95_rtf,
        peak_rss_bytes: peak_rss_bytes(),
        quality,
    };

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", args.output.display());
    Ok(())
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn error_rate<T: Eq>(reference: &[T], hypothesis: &[T]) -> f64 {
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }
    let mut previous = (0..=hypothesis.len()).collect::<Vec<_>>();
    for (reference_index, reference_item) in reference.iter().enumerate() {
        let mut current = vec![reference_index + 1; hypothesis.len() + 1];
        for (hypothesis_index, hypothesis_item) in hypothesis.iter().enumerate() {
            let substitution =
                previous[hypothesis_index] + usize::from(reference_item != hypothesis_item);
            current[hypothesis_index + 1] = (current[hypothesis_index] + 1)
                .min(previous[hypothesis_index + 1] + 1)
                .min(substitution);
        }
        previous = current;
    }
    previous[hypothesis.len()] as f64 / reference.len() as f64
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    u64::try_from(usage.ru_maxrss).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_rates_are_zero_for_equal_text() {
        let words = ["ciao", "mondo"];
        assert_eq!(error_rate(&words, &words), 0.0);
    }

    #[test]
    fn error_rates_count_substitutions() {
        assert_eq!(error_rate(&["ciao", "mondo"], &["ciao", "terra"]), 0.5);
    }
}
