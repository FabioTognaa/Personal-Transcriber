use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::domain::{SessionMetadata, TimestampUs, TranscriptSegment};
use crate::schema::{SCHEMA_VERSION, SESSION_METADATA_FILE, TRANSCRIPT_FILE};
use crate::storage::read_json;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jsonl,
    Text,
    Markdown,
    Srt,
    Vtt,
}

pub fn export_session(session: &Path, format: Format, output: Option<&Path>) -> Result<()> {
    if !session.is_dir() {
        return Err(Error::InvalidPath {
            label: "session",
            path: session.to_path_buf(),
            reason: "expected an existing session directory".to_owned(),
        });
    }
    let transcript = session.join(TRANSCRIPT_FILE);
    ensure_distinct_output(&transcript, output)?;
    let metadata: SessionMetadata = read_json(&session.join(SESSION_METADATA_FILE))?;
    if metadata.schema_version != SCHEMA_VERSION {
        return Err(Error::InvalidExport(format!(
            "unsupported session schema version {}; expected {SCHEMA_VERSION}",
            metadata.schema_version
        )));
    }
    let segments = read_segments(&transcript, metadata.session_id)?;
    let rendered = render(&segments, format)?;

    if let Some(output) = output {
        atomic_write(output, rendered.as_bytes())
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(rendered.as_bytes())?;
        stdout.flush()?;
        Ok(())
    }
}

fn read_segments(path: &Path, expected_session_id: Uuid) -> Result<Vec<TranscriptSegment>> {
    let file = File::open(path)?;
    let mut segments = Vec::new();
    let mut previous_start = TimestampUs(0);

    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let line = line?;
        if line.is_empty() {
            return Err(invalid_transcript(path, line_number, "empty record"));
        }
        let segment: TranscriptSegment = serde_json::from_str(&line).map_err(|error| {
            invalid_transcript(path, line_number, format!("invalid JSON: {error}"))
        })?;
        if segment.schema_version != SCHEMA_VERSION {
            return Err(invalid_transcript(
                path,
                line_number,
                format!(
                    "unsupported schema version {}; expected {SCHEMA_VERSION}",
                    segment.schema_version
                ),
            ));
        }
        if segment.end < segment.start {
            return Err(invalid_transcript(
                path,
                line_number,
                "segment end precedes its start",
            ));
        }
        if !segments.is_empty() && segment.start < previous_start {
            return Err(invalid_transcript(
                path,
                line_number,
                "segment start timestamps are not monotonic",
            ));
        }
        if segment.session_id != expected_session_id {
            return Err(invalid_transcript(
                path,
                line_number,
                "record does not belong to the session metadata",
            ));
        }
        previous_start = segment.start;
        segments.push(segment);
    }

    Ok(segments)
}

fn render(segments: &[TranscriptSegment], format: Format) -> Result<String> {
    match format {
        Format::Jsonl => {
            let mut output = String::new();
            for segment in segments {
                output.push_str(&serde_json::to_string(segment)?);
                output.push('\n');
            }
            Ok(output)
        }
        Format::Text => Ok(render_text(segments)),
        Format::Markdown => Ok(render_markdown(segments)),
        Format::Srt => Ok(render_subtitles(segments, SubtitleFormat::Srt)),
        Format::Vtt => Ok(render_subtitles(segments, SubtitleFormat::Vtt)),
    }
}

fn render_text(segments: &[TranscriptSegment]) -> String {
    let mut output = String::new();
    for segment in segments {
        output.push_str(&segment.text);
        output.push('\n');
    }
    output
}

fn render_markdown(segments: &[TranscriptSegment]) -> String {
    let mut output = String::from("# Transcript\n");
    for segment in segments {
        output.push_str("\n- `");
        output.push_str(&format_timestamp(segment.start, '.'));
        output.push_str(" → ");
        output.push_str(&format_timestamp(segment.end, '.'));
        output.push_str("` ");
        output.push_str(&segment.text);
        output.push('\n');
    }
    output
}

#[derive(Debug, Clone, Copy)]
enum SubtitleFormat {
    Srt,
    Vtt,
}

fn render_subtitles(segments: &[TranscriptSegment], format: SubtitleFormat) -> String {
    let mut output = match format {
        SubtitleFormat::Srt => String::new(),
        SubtitleFormat::Vtt => String::from("WEBVTT\n\n"),
    };
    let separator = match format {
        SubtitleFormat::Srt => ',',
        SubtitleFormat::Vtt => '.',
    };

    for (index, segment) in segments.iter().enumerate() {
        if matches!(format, SubtitleFormat::Srt) {
            output.push_str(&(index + 1).to_string());
            output.push('\n');
        }
        output.push_str(&format_timestamp(segment.start, separator));
        output.push_str(" --> ");
        output.push_str(&format_timestamp(segment.end, separator));
        output.push('\n');
        output.push_str(&segment.text);
        output.push_str("\n\n");
    }
    output
}

fn format_timestamp(timestamp: TimestampUs, separator: char) -> String {
    let total_milliseconds = timestamp.0 / 1_000;
    let milliseconds = total_milliseconds % 1_000;
    let total_seconds = total_milliseconds / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;

    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{milliseconds:03}")
}

fn ensure_distinct_output(transcript: &Path, output: Option<&Path>) -> Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    if output == transcript
        || (output.exists()
            && transcript.exists()
            && fs::canonicalize(output)? == fs::canonicalize(transcript)?)
    {
        return Err(Error::InvalidExport(format!(
            "output cannot overwrite the canonical transcript: {}",
            transcript.display()
        )));
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(Error::InvalidExport(format!(
            "output directory does not exist: {}",
            parent.display()
        )));
    }
    let temporary = temporary_path(parent, path);
    let write_result = (|| -> Result<()> {
        let mut file = File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn temporary_path(parent: &Path, destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()))
}

fn invalid_transcript(path: &Path, line: usize, reason: impl Into<String>) -> Error {
    Error::InvalidTranscript {
        path: path.to_path_buf(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::InferenceConfig;

    fn segment(start: u64, end: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            schema_version: SCHEMA_VERSION,
            segment_id: Uuid::new_v4(),
            session_id: Uuid::nil(),
            start: TimestampUs(start),
            end: TimestampUs(end),
            text: text.to_owned(),
            language: "it".to_owned(),
            model: "fixture.bin".to_owned(),
            model_sha256: "fixture-sha256".to_owned(),
            inference: InferenceConfig::default(),
        }
    }

    #[test]
    fn text_export_preserves_asr_text_verbatim() {
        let rendered = render(
            &[
                segment(0, 1_000_000, " Testo  non corretto."),
                segment(1_500_000, 2_000_000, "Seconda riga"),
            ],
            Format::Text,
        )
        .expect("text should render");

        assert_eq!(rendered, " Testo  non corretto.\nSeconda riga\n");
    }

    #[test]
    fn subtitle_formats_use_their_required_timestamp_separator() {
        let segments = [segment(3_723_456_789, 3_725_000_000, "Ciao")];

        assert_eq!(
            render(&segments, Format::Srt).expect("SRT should render"),
            "1\n01:02:03,456 --> 01:02:05,000\nCiao\n\n"
        );
        assert_eq!(
            render(&segments, Format::Vtt).expect("VTT should render"),
            "WEBVTT\n\n01:02:03.456 --> 01:02:05.000\nCiao\n\n"
        );
    }

    #[test]
    fn malformed_json_reports_its_line_number() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        let transcript = temporary.path().join(TRANSCRIPT_FILE);
        let valid = serde_json::to_string(&segment(0, 1_000_000, "Prima riga"))
            .expect("fixture should serialize");
        fs::write(&transcript, format!("{valid}\nnot-json\n")).expect("fixture should be written");

        let error =
            read_segments(&transcript, Uuid::nil()).expect_err("invalid transcript should fail");

        assert!(matches!(error, Error::InvalidTranscript { line: 2, .. }));
    }

    #[test]
    fn export_refuses_to_overwrite_the_canonical_transcript() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        let transcript = temporary.path().join(TRANSCRIPT_FILE);
        fs::write(&transcript, "").expect("fixture should be written");

        let error = export_session(temporary.path(), Format::Jsonl, Some(transcript.as_path()))
            .expect_err("canonical transcript must be protected");

        assert!(matches!(error, Error::InvalidExport(_)));
    }
}
