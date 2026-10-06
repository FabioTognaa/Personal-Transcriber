use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use live_transcript::Result;
use live_transcript::control;
use live_transcript::domain::{
    ControlAction, SegmenterConfig, SessionEvent, SessionState, SessionStatus, SpeechSegment,
    TimestampUs,
};
use live_transcript::schema::{EVENTS_FILE, MIXED_AUDIO_FILE};
use live_transcript::session::{self, SegmentSink, StartOptions};

const FIXTURE: &str = "tests/fixtures/m1_stream.wav";

#[derive(Clone)]
struct RecordingSink {
    ranges: Arc<Mutex<Vec<(TimestampUs, TimestampUs)>>>,
}

impl SegmentSink for RecordingSink {
    fn accept(&mut self, segment: &SpeechSegment) -> Result<()> {
        self.ranges
            .lock()
            .expect("sink mutex poisoned")
            .push((segment.start, segment.end));
        Ok(())
    }
}

struct SlowSink;

impl SegmentSink for SlowSink {
    fn accept(&mut self, _segment: &SpeechSegment) -> Result<()> {
        thread::sleep(Duration::from_millis(100));
        Ok(())
    }
}

#[test]
fn realtime_session_persists_all_audio_and_honors_pause_resume() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let accepted = Arc::new(Mutex::new(Vec::new()));
    let worker_accepted = Arc::clone(&accepted);
    let worker_sessions = sessions_dir.clone();
    let worker = thread::spawn(move || {
        let mut sink = RecordingSink {
            ranges: worker_accepted,
        };
        session::run_with_sink(options(worker_sessions), &mut sink)
    });

    wait_for_position(&sessions_dir, 100_000);
    let paused = control::request(&sessions_dir, ControlAction::Pause)
        .expect("pause should be acknowledged");
    assert_eq!(paused.state, SessionState::TranscriptionPaused);
    wait_for_position(&sessions_dir, 450_000);
    let resumed = control::request(&sessions_dir, ControlAction::Resume)
        .expect("resume should be acknowledged");
    assert_eq!(resumed.state, SessionState::Running);

    let root = worker
        .join()
        .expect("session worker should not panic")
        .expect("session should complete");
    let status = control::current_status(&sessions_dir).expect("status should remain readable");
    assert_eq!(status.state, SessionState::Completed);
    assert_eq!(status.audio_position, TimestampUs(1_200_000));
    assert_eq!(status.chunks_written, 60);
    assert!(status.max_queue_depth <= status.queue_capacity);
    assert!(status.max_segmentation_queue_depth <= status.segmentation_queue_capacity);
    assert!(!status.segmentation_replay_required);
    assert!(status.segmentation.segments_finalized > 0);

    let output = hound::WavReader::open(root.join("audio").join(MIXED_AUDIO_FILE))
        .expect("output WAV should be readable");
    assert_eq!(output.spec().sample_rate, 16_000);
    assert_eq!(output.spec().channels, 1);
    assert_eq!(output.duration(), 19_200);

    let events = read_events(&root.join(EVENTS_FILE));
    let pause_at = event_timestamp(&events, "pause");
    let resume_at = event_timestamp(&events, "resume");
    assert!(pause_at < resume_at);

    let accepted = accepted.lock().expect("sink mutex poisoned");
    assert!(!accepted.is_empty());
    assert!(
        accepted
            .iter()
            .all(|(start, end)| end.0 <= pause_at.0 || start.0 >= resume_at.0),
        "segments must not span the transcription pause"
    );
    assert!(
        accepted.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "speech segment timestamps must be monotonic"
    );
    assert!(
        fs::read_to_string(root.join("transcript.jsonl"))
            .expect("transcript should be readable")
            .is_empty(),
        "M1 must not invent ASR output"
    );
}

#[test]
fn stop_is_idempotent_and_a_new_session_does_not_corrupt_the_previous_wav() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let worker_sessions = sessions_dir.clone();
    let worker = thread::spawn(move || session::run(options(worker_sessions)));

    wait_for_position(&sessions_dir, 100_000);
    control::request(&sessions_dir, ControlAction::Stop).expect("stop should be acknowledged");
    let first_root = worker
        .join()
        .expect("session worker should not panic")
        .expect("stopped session should finalize");
    let first_audio = first_root.join("audio").join(MIXED_AUDIO_FILE);
    let first_samples = wav_samples(&first_audio);
    assert!(first_samples > 0);
    assert!(first_samples < 19_200);

    let stopped_again =
        control::request(&sessions_dir, ControlAction::Stop).expect("second stop is idempotent");
    assert_eq!(stopped_again.state, SessionState::Completed);

    let second_root = session::run(options(sessions_dir)).expect("a new session should run");
    assert_ne!(first_root, second_root);
    assert_eq!(wav_samples(&first_audio), first_samples);
    assert_eq!(
        wav_samples(&second_root.join("audio").join(MIXED_AUDIO_FILE)),
        19_200
    );
}

#[test]
fn stop_drains_audio_already_captured_in_the_bounded_queue() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let sessions_dir = temporary.path().join("sessions");
    let worker_sessions = sessions_dir.clone();
    let worker =
        thread::spawn(move || session::run_with_sink(options(worker_sessions), &mut SlowSink));

    let before_stop = wait_for_position(&sessions_dir, 20_000);
    control::request(&sessions_dir, ControlAction::Stop).expect("stop should complete");
    let root = worker
        .join()
        .expect("session worker should not panic")
        .expect("stopped session should finalize");
    let completed = control::current_status(&sessions_dir).expect("status should be readable");

    assert!(completed.chunks_written > before_stop.chunks_written);
    assert_eq!(
        wav_samples(&root.join("audio").join(MIXED_AUDIO_FILE)),
        u32::try_from(completed.audio_position.0 * 16_000 / 1_000_000)
            .expect("fixture duration fits in u32")
    );
}

fn options(sessions_dir: PathBuf) -> StartOptions {
    StartOptions {
        sessions_dir,
        input_wav: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE),
        model: None,
        language: "it".to_owned(),
        microphone: None,
        system_audio: None,
        segmenter: SegmenterConfig::default(),
    }
}

fn wait_for_position(sessions_dir: &Path, target_us: u64) -> SessionStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(status) = control::current_status(sessions_dir)
            && status.audio_position.0 >= target_us
        {
            return status;
        }
        assert!(Instant::now() < deadline, "session did not make progress");
        thread::sleep(Duration::from_millis(5));
    }
}

fn read_events(path: &Path) -> Vec<SessionEvent> {
    fs::read_to_string(path)
        .expect("events should be readable")
        .lines()
        .map(|line| serde_json::from_str(line).expect("event should be valid JSON"))
        .collect()
}

fn event_timestamp(events: &[SessionEvent], kind: &str) -> TimestampUs {
    events
        .iter()
        .find_map(|event| match (kind, event) {
            ("pause", SessionEvent::TranscriptionPaused { at, .. })
            | ("resume", SessionEvent::TranscriptionResumed { at, .. }) => Some(*at),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {kind} event"))
}

fn wav_samples(path: &Path) -> u32 {
    hound::WavReader::open(path)
        .expect("WAV should be readable")
        .duration()
}
