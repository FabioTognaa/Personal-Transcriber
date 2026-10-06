use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use personal_transcriber::Result;
use personal_transcriber::asr::{
    AsrEngine, AsrOutput, TranscribingSink, WhisperEngine, model_identity,
};
use personal_transcriber::audio::FileAudioSource;
use personal_transcriber::domain::{
    InferenceConfig, ModelIdentity, SegmenterConfig, SpeechSegment, TranscriptSegment,
};
use personal_transcriber::session::{self, SegmentSink, StartOptions};
use uuid::Uuid;

const FIXTURE: &str = "tests/fixtures/m2_speech_with_short_pause.wav";
const RAW_TEXT: &str = " Testo ASR  non corretto.";

struct MockAsrEngine;

impl AsrEngine for MockAsrEngine {
    fn transcribe(
        &mut self,
        _segment: &SpeechSegment,
        _language: &str,
        _config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        Ok(AsrOutput {
            text: RAW_TEXT.to_owned(),
            inference_duration_us: 10_000,
        })
    }
}

struct FailingAsrEngine;

impl AsrEngine for FailingAsrEngine {
    fn transcribe(
        &mut self,
        _segment: &SpeechSegment,
        _language: &str,
        _config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        Err(personal_transcriber::Error::Asr(
            "fixture inference failure".to_owned(),
        ))
    }
}

struct DelayedFirstAsrEngine {
    first: bool,
}

impl AsrEngine for DelayedFirstAsrEngine {
    fn transcribe(
        &mut self,
        _segment: &SpeechSegment,
        _language: &str,
        _config: &InferenceConfig,
    ) -> Result<AsrOutput> {
        if self.first {
            self.first = false;
            std::thread::sleep(Duration::from_secs(2));
        }
        Ok(AsrOutput {
            text: "fixture".to_owned(),
            inference_duration_us: 1,
        })
    }
}

#[test]
fn simulated_english_session_writes_verbatim_canonical_transcript() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let model = fixture_model();
    let inference = InferenceConfig::default();
    let mut sink = TranscribingSink::new(
        MockAsrEngine,
        model.clone(),
        "en".to_owned(),
        inference.clone(),
    );

    let root = session::run_with_sink(
        StartOptions {
            sessions_dir: sessions_dir.clone(),
            input_wav: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)),
            model: Some(model.path.clone()),
            language: "en".to_owned(),
            microphone: None,
            system_audio: None,
            segmenter: SegmenterConfig::default(),
            inference: inference.clone(),
            model_identity: Some(model.clone()),
        },
        &mut sink,
    )
    .expect("simulated ASR session should complete");

    let lines =
        fs::read_to_string(root.join("transcript.jsonl")).expect("transcript should be readable");
    let segments = lines
        .lines()
        .map(|line| serde_json::from_str::<TranscriptSegment>(line).expect("valid segment"))
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, RAW_TEXT);
    assert_eq!(segments[0].language, "en");
    assert_eq!(segments[0].model, model.name);
    assert_eq!(segments[0].model_sha256, model.sha256);
    assert_eq!(segments[0].inference, inference);

    let status = personal_transcriber::control::current_status(&sessions_dir)
        .expect("final status should be readable");
    assert_eq!(status.asr.segments_transcribed, 1);
    assert_eq!(status.asr_queue_depth, 0);
    assert!(status.asr.real_time_factor_milli > 0);
}

#[test]
fn duplicate_segment_ranges_are_not_appended_twice() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let transcript = temporary.path().join("transcript.jsonl");
    fs::File::create(&transcript).expect("transcript should be created");
    let mut sink = TranscribingSink::new(
        MockAsrEngine,
        fixture_model(),
        "it".to_owned(),
        InferenceConfig::default(),
    );
    sink.start(Uuid::nil(), &transcript)
        .expect("sink should start");
    let segment = SpeechSegment {
        start: personal_transcriber::domain::TimestampUs(0),
        end: personal_transcriber::domain::TimestampUs(200_000),
        samples: vec![0.25; 3_200],
    };
    sink.accept(&segment).expect("first segment should queue");
    sink.accept(&segment)
        .expect("duplicate segment should queue");
    sink.finish().expect("sink should finish");

    assert_eq!(
        fs::read_to_string(transcript)
            .expect("transcript should be readable")
            .lines()
            .count(),
        1
    );
}

#[test]
fn worker_reports_the_original_inference_error() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let transcript = temporary.path().join("transcript.jsonl");
    fs::File::create(&transcript).expect("transcript should be created");
    let mut sink = TranscribingSink::new(
        FailingAsrEngine,
        fixture_model(),
        "it".to_owned(),
        InferenceConfig::default(),
    );
    sink.start(Uuid::nil(), &transcript)
        .expect("sink should start");
    sink.accept(&SpeechSegment {
        start: personal_transcriber::domain::TimestampUs(0),
        end: personal_transcriber::domain::TimestampUs(200_000),
        samples: vec![0.25; 3_200],
    })
    .expect("segment should queue");

    let error = sink.finish().expect_err("inference should fail");
    assert!(matches!(
        error,
        personal_transcriber::Error::Asr(message) if message == "fixture inference failure"
    ));
}

#[test]
fn session_stops_when_the_asr_worker_fails() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let model = fixture_model();
    let inference = InferenceConfig::default();
    let segmenter = SegmenterConfig {
        max_segment_ms: 200,
        ..SegmenterConfig::default()
    };
    let mut sink = TranscribingSink::new(
        FailingAsrEngine,
        model.clone(),
        "it".to_owned(),
        inference.clone(),
    );

    let started = Instant::now();
    let error = session::run_with_sink(
        StartOptions {
            sessions_dir: sessions_dir.clone(),
            input_wav: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)),
            model: Some(model.path.clone()),
            language: "it".to_owned(),
            microphone: None,
            system_audio: None,
            segmenter,
            inference,
            model_identity: Some(model),
        },
        &mut sink,
    )
    .expect_err("ASR failure should stop the session");

    assert!(
        matches!(error, personal_transcriber::Error::Asr(message) if message == "fixture inference failure")
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let status = personal_transcriber::control::current_status(&sessions_dir)
        .expect("failed status should be readable");
    assert_eq!(
        status.state,
        personal_transcriber::domain::SessionState::Failed
    );
    assert!(status.audio_position.0 < 2_200_000);
}

#[test]
fn segmentation_replay_does_not_duplicate_transcript_ranges() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let input = temporary.path().join("continuous-speech.wav");
    write_constant_wav(&input, 3);
    let model = fixture_model();
    let inference = InferenceConfig::default();
    let segmenter = SegmenterConfig {
        energy_threshold: 0.02,
        start_trigger_ms: 20,
        end_silence_ms: 20,
        pre_roll_ms: 0,
        post_roll_ms: 0,
        min_speech_ms: 20,
        max_segment_ms: 40,
    };
    let mut sink = TranscribingSink::new(
        DelayedFirstAsrEngine { first: true },
        model.clone(),
        "it".to_owned(),
        inference.clone(),
    );

    let root = session::run_with_sink(
        StartOptions {
            sessions_dir: sessions_dir.clone(),
            input_wav: Some(input),
            model: Some(model.path.clone()),
            language: "it".to_owned(),
            microphone: None,
            system_audio: None,
            segmenter,
            inference,
            model_identity: Some(model),
        },
        &mut sink,
    )
    .expect("overloaded session should recover by replaying segmentation");

    let transcript =
        fs::read_to_string(root.join("transcript.jsonl")).expect("transcript should be readable");
    let ranges = transcript
        .lines()
        .map(|line| {
            let segment: TranscriptSegment =
                serde_json::from_str(line).expect("segment should be valid");
            (segment.start, segment.end)
        })
        .collect::<Vec<_>>();
    let unique = ranges
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let status = personal_transcriber::control::current_status(&sessions_dir)
        .expect("status should be readable");

    assert!(status.segmentation_replay_required);
    assert!(!ranges.is_empty());
    assert_eq!(ranges.len(), unique.len());
}

#[test]
#[ignore = "requires PERSONAL_TRANSCRIBER_MODEL and a local GGML model"]
fn real_whisper_model_processes_local_audio() {
    let model_path = PathBuf::from(
        std::env::var("PERSONAL_TRANSCRIBER_MODEL")
            .expect("PERSONAL_TRANSCRIBER_MODEL must point to a GGML model"),
    );
    let config = InferenceConfig::default();
    let mut engine = WhisperEngine::load(&model_path, &config).expect("model should load");
    let source = FileAudioSource::open(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE))
        .expect("fixture should load");
    let samples = source
        .chunks()
        .flat_map(|chunk| chunk.samples)
        .collect::<Vec<_>>();
    let segment = SpeechSegment {
        start: personal_transcriber::domain::TimestampUs(0),
        end: personal_transcriber::domain::TimestampUs(2_200_000),
        samples,
    };

    engine
        .transcribe(&segment, "it", &config)
        .expect("whisper inference should succeed");
    model_identity(&model_path).expect("model identity should be readable");
}

fn fixture_model() -> ModelIdentity {
    ModelIdentity {
        path: PathBuf::from("models/mock.bin"),
        name: "mock.bin".to_owned(),
        sha256: "fixture-sha256".to_owned(),
        size_bytes: 1,
    }
}

fn write_constant_wav(path: &std::path::Path, seconds: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("fixture WAV should be created");
    for _ in 0..seconds * spec.sample_rate {
        writer
            .write_sample(i16::MAX / 4)
            .expect("fixture sample should be written");
    }
    writer.finalize().expect("fixture WAV should finalize");
}
