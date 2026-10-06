use std::fs;
use std::path::PathBuf;

use live_transcript::Result;
use live_transcript::asr::{AsrEngine, AsrOutput, TranscribingSink, WhisperEngine, model_identity};
use live_transcript::audio::FileAudioSource;
use live_transcript::domain::{
    InferenceConfig, ModelIdentity, SegmenterConfig, SpeechSegment, TranscriptSegment,
};
use live_transcript::session::{self, SegmentSink, StartOptions};
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
        Err(live_transcript::Error::Asr(
            "fixture inference failure".to_owned(),
        ))
    }
}

#[test]
fn simulated_session_writes_verbatim_canonical_transcript() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let model = fixture_model();
    let inference = InferenceConfig::default();
    let mut sink = TranscribingSink::new(
        MockAsrEngine,
        model.clone(),
        "it".to_owned(),
        inference.clone(),
    );

    let root = session::run_with_sink(
        StartOptions {
            sessions_dir: sessions_dir.clone(),
            input_wav: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)),
            model: Some(model.path.clone()),
            language: "it".to_owned(),
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
    assert_eq!(segments[0].language, "it");
    assert_eq!(segments[0].model, model.name);
    assert_eq!(segments[0].model_sha256, model.sha256);
    assert_eq!(segments[0].inference, inference);

    let status = live_transcript::control::current_status(&sessions_dir)
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
        start: live_transcript::domain::TimestampUs(0),
        end: live_transcript::domain::TimestampUs(200_000),
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
        start: live_transcript::domain::TimestampUs(0),
        end: live_transcript::domain::TimestampUs(200_000),
        samples: vec![0.25; 3_200],
    })
    .expect("segment should queue");

    let error = sink.finish().expect_err("inference should fail");
    assert!(matches!(
        error,
        live_transcript::Error::Asr(message) if message == "fixture inference failure"
    ));
}

#[test]
#[ignore = "requires LIVE_TRANSCRIPT_MODEL and a local GGML model"]
fn real_whisper_model_processes_local_audio() {
    let model_path = PathBuf::from(
        std::env::var("LIVE_TRANSCRIPT_MODEL")
            .expect("LIVE_TRANSCRIPT_MODEL must point to a GGML model"),
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
        start: live_transcript::domain::TimestampUs(0),
        end: live_transcript::domain::TimestampUs(2_200_000),
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
