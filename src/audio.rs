use std::path::Path;

use crate::domain::{
    AudioSourceKind, PcmChunk, TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ, TimestampUs,
};
use crate::{Error, Result};

pub const DEFAULT_CHUNK_DURATION_MS: u64 = 20;
const MAX_FILE_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILE_SOURCE_DURATION_SECS: u64 = 30 * 60;

#[derive(Debug, Clone)]
pub struct FileAudioSource {
    samples: Vec<f32>,
    chunk_frames: usize,
}

impl FileAudioSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file_size = std::fs::metadata(path)?.len();
        if file_size > MAX_FILE_SOURCE_BYTES {
            return Err(Error::UnsupportedWav(format!(
                "file is {file_size} bytes; the in-memory simulator is limited to {MAX_FILE_SOURCE_BYTES} bytes"
            )));
        }
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(Error::UnsupportedWav(
                "sample rate and channel count must be non-zero".to_owned(),
            ));
        }
        if u64::from(reader.duration())
            > u64::from(spec.sample_rate).saturating_mul(MAX_FILE_SOURCE_DURATION_SECS)
        {
            return Err(Error::UnsupportedWav(format!(
                "duration exceeds the in-memory simulator limit of {MAX_FILE_SOURCE_DURATION_SECS} seconds"
            )));
        }

        let interleaved = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, 32) => reader
                .samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()?,
            (hound::SampleFormat::Int, 8) => reader
                .samples::<i8>()
                .map(|sample| sample.map(|value| f32::from(value) / 128.0))
                .collect::<std::result::Result<Vec<_>, _>>()?,
            (hound::SampleFormat::Int, 16) => reader
                .samples::<i16>()
                .map(|sample| sample.map(|value| f32::from(value) / 32_768.0))
                .collect::<std::result::Result<Vec<_>, _>>()?,
            (hound::SampleFormat::Int, bits @ (24 | 32)) => {
                let scale = (1_u64 << (bits - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| sample.map(|value| value as f32 / scale))
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (format, bits) => {
                return Err(Error::UnsupportedWav(format!(
                    "{format:?} samples with {bits} bits are not supported"
                )));
            }
        };

        let mono = downmix(&interleaved, usize::from(spec.channels));
        let samples = resample_linear(&mono, spec.sample_rate, TARGET_SAMPLE_RATE_HZ);
        let chunk_frames =
            (u64::from(TARGET_SAMPLE_RATE_HZ) * DEFAULT_CHUNK_DURATION_MS / 1_000) as usize;

        Ok(Self {
            samples,
            chunk_frames,
        })
    }

    pub fn chunks(&self) -> impl Iterator<Item = PcmChunk> + '_ {
        self.samples
            .chunks(self.chunk_frames)
            .enumerate()
            .map(|(index, samples)| PcmChunk {
                source: AudioSourceKind::File,
                start: TimestampUs(
                    (index * self.chunk_frames) as u64 * 1_000_000
                        / u64::from(TARGET_SAMPLE_RATE_HZ),
                ),
                sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
                channels: TARGET_CHANNELS,
                samples: samples.to_vec(),
            })
    }

    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// Streams a canonical 16-bit mono 16 kHz WAV as 20 ms chunks without loading
/// it into memory. Unlike [`FileAudioSource`] it has no size or duration cap,
/// so it can replay audio from sessions longer than the WAV simulator allows.
pub struct StreamingAudioSource {
    reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    chunk_frames: usize,
    index: usize,
}

impl StreamingAudioSource {
    pub fn open(path: &Path) -> Result<Self> {
        let reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.channels != TARGET_CHANNELS
            || spec.sample_rate != TARGET_SAMPLE_RATE_HZ
            || spec.sample_format != hound::SampleFormat::Int
            || spec.bits_per_sample != 16
        {
            return Err(Error::UnsupportedWav(
                "session audio must be 16-bit mono PCM at 16 kHz".to_owned(),
            ));
        }
        let chunk_frames =
            (u64::from(TARGET_SAMPLE_RATE_HZ) * DEFAULT_CHUNK_DURATION_MS / 1_000) as usize;
        Ok(Self {
            reader,
            chunk_frames,
            index: 0,
        })
    }

    pub fn next_chunk(&mut self) -> Result<Option<PcmChunk>> {
        let mut samples = Vec::with_capacity(self.chunk_frames);
        for sample in self.reader.samples::<i16>().take(self.chunk_frames) {
            samples.push(f32::from(sample?) / 32_768.0);
        }
        if samples.is_empty() {
            return Ok(None);
        }
        let start = TimestampUs(
            (self.index * self.chunk_frames) as u64 * 1_000_000 / u64::from(TARGET_SAMPLE_RATE_HZ),
        );
        self.index += 1;
        Ok(Some(PcmChunk {
            source: AudioSourceKind::File,
            start,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
            samples,
        }))
    }
}

pub(crate) fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear interpolation between two samples, shared by every resampler.
pub(crate) fn lerp(lower: f32, upper: f32, fraction: f32) -> f32 {
    lower + (upper - lower) * fraction
}

fn resample_linear(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if input.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return input.to_vec();
    }

    let output_len =
        ((input.len() as u128 * u128::from(target_rate)) / u128::from(source_rate)) as usize;
    let ratio = f64::from(source_rate) / f64::from(target_rate);

    (0..output_len)
        .map(|output_index| {
            let source_position = output_index as f64 * ratio;
            let lower = source_position.floor() as usize;
            let upper = (lower + 1).min(input.len() - 1);
            let fraction = (source_position - lower as f64) as f32;
            lerp(input[lower], input[upper], fraction)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, -1.0, 0.5, 0.5], 2), vec![0.0, 0.5]);
    }

    #[test]
    fn resampling_preserves_expected_duration() {
        let input = vec![0.25; 8_000];
        let output = resample_linear(&input, 8_000, 16_000);

        assert_eq!(output.len(), 16_000);
        assert!(output.iter().all(|sample| *sample == 0.25));
    }

    #[test]
    fn simulator_rejects_files_that_exceed_its_memory_bound() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary file should be created");
        temporary
            .as_file()
            .set_len(MAX_FILE_SOURCE_BYTES + 1)
            .expect("sparse fixture should be resized");

        assert!(matches!(
            FileAudioSource::open(temporary.path()),
            Err(Error::UnsupportedWav(_))
        ));
    }

    #[test]
    fn streaming_source_reads_files_the_simulator_rejects() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let path = temporary.path().join("long.wav");
        write_sparse_wav(&path, MAX_FILE_SOURCE_BYTES / 2 + 1);

        assert!(FileAudioSource::open(&path).is_err());

        let mut source = StreamingAudioSource::open(&path).expect("streaming should open");
        for _ in 0..5 {
            let chunk = source
                .next_chunk()
                .expect("chunk should read")
                .expect("chunk should exist");
            assert_eq!(chunk.samples.len(), 320);
        }
    }

    fn write_sparse_wav(path: &Path, frames: u64) {
        use std::io::Write;

        let data_len = frames * 2;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&16_000u32.to_le_bytes());
        header.extend_from_slice(&32_000u32.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(data_len as u32).to_le_bytes());

        let mut file = std::fs::File::create(path).expect("sparse WAV should be created");
        file.write_all(&header).expect("header should be written");
        file.set_len(44 + data_len)
            .expect("file should be extended");
    }
}
