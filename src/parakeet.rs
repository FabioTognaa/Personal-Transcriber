//! NVIDIA Parakeet TDT engine on ONNX Runtime.
//!
//! The bundle is the int8 export published as
//! `istupakov/parakeet-tdt-0.6b-v3-onnx` (CC-BY-4.0). The pipeline mirrors the
//! reference implementation in `onnx-asr`: mel preprocessor, FastConformer
//! encoder, then greedy TDT decoding (token plus duration).

use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::AutoDevicePolicy;
use ort::value::Tensor;

use crate::asr::{AsrEngine, AsrOutput};
use crate::domain::{InferenceConfig, SpeechSegment};
use crate::model::ModelArtifact;
use crate::{Error, Result};

const SAMPLE_RATE: usize = 16_000;
/// Parakeet was trained on 20-30 s windows, but encoder attention memory grows
/// with the square of the input length. Fifteen seconds keeps a live segment well
/// under 4 GiB on this Mac.
const MAX_CHUNK_SECONDS: usize = 15;
const ENCODER_HIDDEN: usize = 1024;
const DECODER_STATE_SHAPE: [usize; 3] = [2, 1, 640];
const MAX_TOKENS_PER_STEP: usize = 10;

static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

fn environment() -> Result<&'static Environment> {
    if let Some(environment) = ENVIRONMENT.get() {
        return Ok(environment);
    }
    let environment = ort::init().build().map_err(asr)?;
    Ok(ENVIRONMENT.get_or_init(|| environment))
}

fn asr(error: impl std::fmt::Display) -> Error {
    Error::Asr(error.to_string())
}

pub struct ParakeetEngine {
    preprocessor: Session,
    encoder: Session,
    decoder_joint: Session,
    vocab: Vec<String>,
    blank: usize,
}

impl ParakeetEngine {
    pub fn load(artifact: &ModelArtifact, config: &InferenceConfig) -> Result<Self> {
        let files = artifact.files()?;
        let preprocessor = files
            .iter()
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("nemo") && name.ends_with(".onnx"))
            })
            .ok_or_else(|| {
                Error::Asr("Parakeet bundle has no nemo*.onnx preprocessor".to_owned())
            })?;
        let vocab_path = artifact.path.join(crate::model::PARAKEET_VOCAB);
        let (vocab, blank) = load_vocab(&vocab_path)?;
        if config
            .prompt
            .as_deref()
            .is_some_and(|prompt| !prompt.is_empty())
        {
            tracing::warn!("Parakeet ignores the initial prompt; use Whisper for a glossary");
        }

        Ok(Self {
            preprocessor: open_session(preprocessor, config.threads)?,
            encoder: open_session(
                &artifact.path.join(crate::model::PARAKEET_ENCODER),
                config.threads,
            )?,
            decoder_joint: open_session(
                &artifact.path.join(crate::model::PARAKEET_DECODER_JOINT),
                config.threads,
            )?,
            vocab,
            blank,
        })
    }

    fn transcribe_chunk(&mut self, samples: &[f32]) -> Result<String> {
        let sample_count = samples.len();
        if sample_count == 0 {
            return Ok(String::new());
        }

        let preprocessed = self
            .preprocessor
            .run(ort::inputs![
                "waveforms" => Tensor::from_array((vec![1_usize, sample_count], samples.to_vec())).map_err(asr)?,
                "waveforms_lens" => Tensor::from_array((vec![1_usize], vec![sample_count as i64])).map_err(asr)?,
            ])
            .map_err(asr)?;
        let (feature_shape, feature_data) = preprocessed["features"]
            .try_extract_tensor::<f32>()
            .map_err(asr)?;
        let feature_time = feature_shape[2] as usize;
        let features = feature_data.to_vec();
        let (_, feature_lens) = preprocessed["features_lens"]
            .try_extract_tensor::<i64>()
            .map_err(asr)?;
        let feature_len = feature_lens[0] as usize;
        drop(preprocessed);

        let encoded = self
            .encoder
            .run(ort::inputs![
                "audio_signal" => Tensor::from_array((vec![1_usize, 128, feature_time], features)).map_err(asr)?,
                "length" => Tensor::from_array((vec![1_usize], vec![feature_len as i64])).map_err(asr)?,
            ])
            .map_err(asr)?;
        let (encoded_shape, encoded_data) = encoded["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(asr)?;
        let encoded_time = encoded_shape[2] as usize;
        let encoded_values = encoded_data.to_vec();
        let (_, encoded_lens) = encoded["encoded_lengths"]
            .try_extract_tensor::<i64>()
            .map_err(asr)?;
        let encoded_len = (encoded_lens[0] as usize).min(encoded_time);
        drop(encoded);

        let mut state_1 = vec![0.0_f32; DECODER_STATE_SHAPE.iter().product()];
        let mut state_2 = vec![0.0_f32; DECODER_STATE_SHAPE.iter().product()];
        let mut tokens: Vec<usize> = Vec::new();
        let mut previous = self.blank;
        let mut frame = 0_usize;
        let mut emitted = 0_usize;
        let mut frame_buffer = vec![0.0_f32; ENCODER_HIDDEN];

        while frame < encoded_len {
            for (hidden, value) in frame_buffer.iter_mut().enumerate() {
                *value = encoded_values[hidden * encoded_time + frame];
            }
            let output = self
                .decoder_joint
                .run(ort::inputs![
                    "encoder_outputs" => Tensor::from_array((vec![1_usize, ENCODER_HIDDEN, 1_usize], frame_buffer.clone())).map_err(asr)?,
                    "targets" => Tensor::from_array((vec![1_usize, 1_usize], vec![previous as i32])).map_err(asr)?,
                    "target_length" => Tensor::from_array((vec![1_usize], vec![1_i32])).map_err(asr)?,
                    "input_states_1" => Tensor::from_array((DECODER_STATE_SHAPE.to_vec(), state_1.clone())).map_err(asr)?,
                    "input_states_2" => Tensor::from_array((DECODER_STATE_SHAPE.to_vec(), state_2.clone())).map_err(asr)?,
                ])
                .map_err(asr)?;
            let (_, logits) = output["outputs"].try_extract_tensor::<f32>().map_err(asr)?;
            let token = argmax(&logits[..self.vocab.len()]);
            let duration = argmax(&logits[self.vocab.len()..]);
            let next_state_1 = output["output_states_1"]
                .try_extract_tensor::<f32>()
                .map_err(asr)?
                .1
                .to_vec();
            let next_state_2 = output["output_states_2"]
                .try_extract_tensor::<f32>()
                .map_err(asr)?
                .1
                .to_vec();
            drop(output);

            if token != self.blank {
                state_1 = next_state_1;
                state_2 = next_state_2;
                tokens.push(token);
                previous = token;
                emitted += 1;
            }

            if duration > 0 {
                frame += duration;
                emitted = 0;
            } else if token == self.blank || emitted >= MAX_TOKENS_PER_STEP {
                frame += 1;
                emitted = 0;
            }
        }

        Ok(self.detokenize(&tokens))
    }

    fn detokenize(&self, tokens: &[usize]) -> String {
        let mut joined = String::new();
        for &token in tokens {
            if let Some(text) = self.vocab.get(token) {
                joined.push_str(text);
            }
        }
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

impl AsrEngine for ParakeetEngine {
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        _language: &str,
        _config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        let started = Instant::now();
        let chunk_samples = MAX_CHUNK_SECONDS * SAMPLE_RATE;
        let mut text = String::new();
        for chunk in segment.samples.chunks(chunk_samples) {
            let part = self.transcribe_chunk(chunk)?;
            if !part.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&part);
            }
        }
        Ok(AsrOutput {
            text,
            inference_duration_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        })
    }
}

fn open_session(path: &Path, threads: i32) -> Result<Session> {
    Session::builder(environment()?)
        .map_err(asr)?
        // The default device policy registers CoreML on macOS; the first
        // iteration is CPU-only by decision, and the CPU arena inflates the peak
        // because input lengths vary with every segment.
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(asr)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(asr)?
        .with_memory_pattern(false)
        .map_err(asr)?
        .with_intra_threads(usize::try_from(threads).unwrap_or(1))
        .map_err(asr)?
        .commit_from_file(path)
        .map_err(asr)
}

fn load_vocab(path: &Path) -> Result<(Vec<String>, usize)> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::Asr(format!(
            "cannot read Parakeet vocabulary {}: {error}",
            path.display()
        ))
    })?;
    let mut vocab: Vec<String> = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (token, id) = line
            .rsplit_once(' ')
            .ok_or_else(|| Error::Asr(format!("invalid Parakeet vocabulary line: {line}")))?;
        let id: usize = id
            .parse()
            .map_err(|_| Error::Asr(format!("invalid Parakeet vocabulary id: {id}")))?;
        if vocab.len() <= id {
            vocab.resize(id + 1, String::new());
        }
        vocab[id] = token.replace('\u{2581}', " ");
    }
    let blank = vocab
        .iter()
        .position(|token| token == "<blk>")
        .ok_or_else(|| Error::Asr("Parakeet vocabulary has no <blk> token".to_owned()))?;
    Ok((vocab, blank))
}

fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .map_or(0, |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TimestampUs;

    #[test]
    fn vocabulary_is_parsed_by_id_and_spaces_are_restored() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let path = temporary.path().join("vocab.txt");
        std::fs::write(&path, "<blk> 0\n\u{2581}ciao 1\nmondo 2\n").expect("fixture");

        let (vocab, blank) = load_vocab(&path).expect("vocabulary should parse");

        assert_eq!(blank, 0);
        assert_eq!(vocab[1], " ciao");
        assert_eq!(vocab[2], "mondo");
    }

    #[test]
    fn argmax_returns_the_highest_index() {
        assert_eq!(argmax(&[0.1, 0.9, 0.3]), 1);
        assert_eq!(argmax(&[]), 0);
    }

    /// Release output matches the human reference; debug builds can differ by a
    /// word in int8 kernels, so this checks the substance instead of an exact
    /// string. Run with `SCRIB_PARAKEET_DIR` and `--ignored`.
    #[test]
    #[ignore = "requires the Parakeet bundle; set SCRIB_PARAKEET_DIR and run with --ignored"]
    fn italian_fixture_transcribes_correctly() {
        let directory = std::env::var("SCRIB_PARAKEET_DIR")
            .unwrap_or_else(|_| "models/parakeet-tdt-0.6b-v3-onnx".to_owned());
        let artifact = ModelArtifact::detect(Path::new(&directory)).expect("Parakeet bundle");
        let mut engine =
            ParakeetEngine::load(&artifact, &InferenceConfig::default()).expect("engine loads");

        let mut reader = hound::WavReader::open("benchmarks/fixtures/italian_synthetic.wav")
            .expect("fixture opens");
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE as u32);
        assert_eq!(reader.spec().channels, 1);
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.expect("sample") as f32 / i16::MAX as f32)
            .collect();
        let end = TimestampUs(
            (samples.len() as u64)
                .saturating_mul(1_000_000)
                .saturating_div(SAMPLE_RATE as u64),
        );
        let segment = SpeechSegment {
            start: TimestampUs(0),
            end,
            samples,
        };

        let output = engine
            .transcribe(&segment, "it", &InferenceConfig::default())
            .expect("transcription succeeds");

        assert!(
            output
                .text
                .starts_with("Questa è una prova di trascrizione locale in italiano.")
        );
        assert!(output.text.contains("non invia dati a servizi esterni"));
        assert!(
            output
                .text
                .contains("Durante una pausa la registrazione continua")
        );
        assert!((30..=40).contains(&output.text.split_whitespace().count()));
    }
}
