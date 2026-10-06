use std::path::Path;

use crate::domain::{
    AudioSourceKind, PcmChunk, TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ, TimestampUs,
};
use crate::{Error, Result};

pub const DEFAULT_CHUNK_DURATION_MS: u64 = 20;

#[derive(Debug, Clone)]
pub struct FileAudioSource {
    samples: Vec<f32>,
    chunk_frames: usize,
}

impl FileAudioSource {
    pub fn open(path: &Path) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(Error::UnsupportedWav(
                "sample rate and channel count must be non-zero".to_owned(),
            ));
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

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
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
            input[lower] + (input[upper] - input[lower]) * fraction
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
}
