use std::fs;
use std::path::Path;

use live_transcript::domain::{
    InferenceConfig, SessionConfig, SessionMetadata, SessionState, TimestampUs, TranscriptSegment,
};
use live_transcript::export::{self, Format};
use live_transcript::schema::{SCHEMA_VERSION, SESSION_METADATA_FILE, TRANSCRIPT_FILE};
use uuid::Uuid;

const RAW_TEXT: &str = " Testo ASR  non corretto.";

#[test]
fn every_export_is_derived_from_the_canonical_transcript() {
    let temporary = tempfile::tempdir().expect("temp directory should be created");
    let session = temporary.path().join("session");
    fs::create_dir(&session).expect("session directory should be created");
    write_session(&session);

    let cases = [
        (Format::Jsonl, "jsonl"),
        (Format::Text, "txt"),
        (Format::Markdown, "md"),
        (Format::Srt, "srt"),
        (Format::Vtt, "vtt"),
    ];
    for (format, extension) in cases {
        let output = temporary.path().join(format!("transcript.{extension}"));
        export::export_session(&session, format, Some(&output))
            .expect("canonical transcript should export");
        let rendered = fs::read_to_string(&output).expect("export should be readable");
        assert!(
            rendered.contains(RAW_TEXT),
            "{extension} must preserve ASR text verbatim"
        );

        if format == Format::Jsonl {
            let decoded: TranscriptSegment = serde_json::from_str(
                rendered
                    .lines()
                    .next()
                    .expect("JSONL should contain one record"),
            )
            .expect("exported JSONL should remain canonical");
            assert_eq!(decoded.text, RAW_TEXT);
        }
    }
}

fn write_session(session: &Path) {
    let session_id = Uuid::new_v4();
    let metadata = SessionMetadata {
        schema_version: SCHEMA_VERSION,
        session_id,
        state: SessionState::Completed,
        started_at_unix_ms: 1,
        ended_at_unix_ms: Some(2),
        config: SessionConfig::italian(Some("models/fixture.bin".into())),
    };
    fs::write(
        session.join(SESSION_METADATA_FILE),
        serde_json::to_vec_pretty(&metadata).expect("metadata should serialize"),
    )
    .expect("metadata should be written");

    let segment = TranscriptSegment {
        schema_version: SCHEMA_VERSION,
        segment_id: Uuid::new_v4(),
        session_id,
        start: TimestampUs(1_234_000),
        end: TimestampUs(2_345_000),
        text: RAW_TEXT.to_owned(),
        language: "it".to_owned(),
        model: "fixture.bin".to_owned(),
        model_sha256: "fixture-sha256".to_owned(),
        inference: InferenceConfig::default(),
    };
    fs::write(
        session.join(TRANSCRIPT_FILE),
        format!(
            "{}\n",
            serde_json::to_string(&segment).expect("segment should serialize")
        ),
    )
    .expect("transcript should be written");
}
