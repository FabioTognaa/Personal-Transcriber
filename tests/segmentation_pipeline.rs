use std::path::PathBuf;

use personal_transcriber::audio::FileAudioSource;
use personal_transcriber::domain::{SegmenterConfig, TimestampUs};
use personal_transcriber::segment::Segmenter;

const FIXTURE: &str = "tests/fixtures/m2_speech_with_short_pause.wav";

#[test]
fn fixture_with_short_pause_produces_one_deterministic_segment() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let source = FileAudioSource::open(&fixture).expect("fixture should be readable");
    let mut segmenter = Segmenter::new(SegmenterConfig::default()).expect("valid config");

    let mut segments = source
        .chunks()
        .flat_map(|chunk| segmenter.push_chunk(chunk))
        .collect::<Vec<_>>();
    segments.extend(segmenter.flush());

    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].start, TimestampUs(0));
    // With the 1600 ms end-silence the 1000 ms trailing pause no longer closes
    // the segment during streaming; flush() finalizes the whole span.
    assert_eq!(segments[0].end, TimestampUs(2_200_000));
    assert_eq!(segments[0].samples.len(), 35_200);

    let metrics = segmenter.metrics();
    assert_eq!(metrics.speech_duration_us, 600_000);
    assert_eq!(metrics.silence_duration_us, 1_600_000);
    assert_eq!(metrics.segments_finalized, 1);
    assert_eq!(metrics.segments_discarded, 0);
}
