use std::collections::VecDeque;

use crate::domain::{
    PcmChunk, SegmentationMetrics, SegmenterConfig, SpeechSegment, TARGET_CHANNELS,
    TARGET_SAMPLE_RATE_HZ, TimestampUs,
};
use crate::{Error, Result};

#[derive(Debug)]
struct ActiveSegment {
    start: TimestampUs,
    samples: Vec<f32>,
    speech_duration_us: u64,
    last_voice_end: TimestampUs,
    trailing_silence_us: u64,
}

#[derive(Debug)]
pub struct Segmenter {
    config: SegmenterConfig,
    pre_roll: VecDeque<PcmChunk>,
    active: Option<ActiveSegment>,
    voice_run_us: u64,
    first_voice_start: Option<TimestampUs>,
    metrics: SegmentationMetrics,
}

impl Segmenter {
    pub fn new(config: SegmenterConfig) -> Result<Self> {
        validate_config(&config)?;
        Ok(Self {
            config,
            pre_roll: VecDeque::new(),
            active: None,
            voice_run_us: 0,
            first_voice_start: None,
            metrics: SegmentationMetrics::default(),
        })
    }

    #[must_use]
    pub fn metrics(&self) -> SegmentationMetrics {
        self.metrics
    }

    pub fn push_chunk(&mut self, chunk: PcmChunk) -> Vec<SpeechSegment> {
        debug_assert_eq!(chunk.sample_rate_hz, TARGET_SAMPLE_RATE_HZ);
        debug_assert_eq!(chunk.channels, TARGET_CHANNELS);

        let voiced = rms(&chunk.samples) >= self.config.energy_threshold;
        let duration_us = chunk.duration_us();
        if voiced {
            self.metrics.speech_duration_us =
                self.metrics.speech_duration_us.saturating_add(duration_us);
        } else {
            self.metrics.silence_duration_us =
                self.metrics.silence_duration_us.saturating_add(duration_us);
        }

        if self.active.is_some() {
            return self.push_active(chunk, voiced);
        }

        self.push_pre_roll(chunk.clone());
        if voiced {
            if self.first_voice_start.is_none() {
                self.first_voice_start = Some(chunk.start);
            }
            self.voice_run_us = self.voice_run_us.saturating_add(duration_us);
        } else {
            self.voice_run_us = 0;
            self.first_voice_start = None;
        }

        if self.voice_run_us >= millis_to_micros(self.config.start_trigger_ms) {
            self.activate_from_pre_roll();
        }

        Vec::new()
    }

    pub fn flush(&mut self) -> Vec<SpeechSegment> {
        if self.active.is_none() && self.voice_run_us > 0 {
            self.metrics.segments_discarded = self.metrics.segments_discarded.saturating_add(1);
        }
        self.voice_run_us = 0;
        self.first_voice_start = None;
        self.pre_roll.clear();
        self.finalize_active(None, false).into_iter().collect()
    }

    fn push_active(&mut self, chunk: PcmChunk, voiced: bool) -> Vec<SpeechSegment> {
        let chunk_end = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
        let active = self.active.as_mut().expect("active segment exists");
        active.samples.extend_from_slice(&chunk.samples);
        if voiced {
            active.speech_duration_us = active
                .speech_duration_us
                .saturating_add(chunk.duration_us());
            active.last_voice_end = chunk_end;
            active.trailing_silence_us = 0;
        } else {
            active.trailing_silence_us = active
                .trailing_silence_us
                .saturating_add(chunk.duration_us());
        }

        let segment_duration_us = chunk_end.0.saturating_sub(active.start.0);
        if segment_duration_us >= millis_to_micros(self.config.max_segment_ms) {
            self.metrics.max_duration_splits = self.metrics.max_duration_splits.saturating_add(1);
            return self
                .finalize_active(Some(chunk_end), true)
                .into_iter()
                .collect();
        }

        if active.trailing_silence_us >= millis_to_micros(self.config.end_silence_ms) {
            let desired_end = TimestampUs(
                active
                    .last_voice_end
                    .0
                    .saturating_add(millis_to_micros(self.config.post_roll_ms))
                    .min(chunk_end.0),
            );
            return self
                .finalize_active(Some(desired_end), false)
                .into_iter()
                .collect();
        }

        Vec::new()
    }

    fn push_pre_roll(&mut self, chunk: PcmChunk) {
        self.pre_roll.push_back(chunk);
        let keep_us = millis_to_micros(
            self.config
                .pre_roll_ms
                .saturating_add(self.config.start_trigger_ms),
        );
        while total_duration(&self.pre_roll) > keep_us {
            self.pre_roll.pop_front();
        }
    }

    fn activate_from_pre_roll(&mut self) {
        let first_voice_start = self
            .first_voice_start
            .expect("voice start exists after trigger");
        let desired_start = first_voice_start
            .0
            .saturating_sub(millis_to_micros(self.config.pre_roll_ms));

        while self
            .pre_roll
            .front()
            .is_some_and(|chunk| chunk.start.0.saturating_add(chunk.duration_us()) <= desired_start)
        {
            self.pre_roll.pop_front();
        }

        let start = self
            .pre_roll
            .front()
            .map_or(first_voice_start, |chunk| chunk.start);
        let mut samples = Vec::new();
        let mut last_voice_end = first_voice_start;
        for chunk in self.pre_roll.drain(..) {
            last_voice_end = TimestampUs(chunk.start.0.saturating_add(chunk.duration_us()));
            samples.extend_from_slice(&chunk.samples);
        }
        self.active = Some(ActiveSegment {
            start,
            samples,
            speech_duration_us: self.voice_run_us,
            last_voice_end,
            trailing_silence_us: 0,
        });
        self.voice_run_us = 0;
        self.first_voice_start = None;
    }

    fn finalize_active(
        &mut self,
        requested_end: Option<TimestampUs>,
        split_at_max: bool,
    ) -> Option<SpeechSegment> {
        let mut active = self.active.take()?;
        if active.speech_duration_us < millis_to_micros(self.config.min_speech_ms) {
            self.metrics.segments_discarded = self.metrics.segments_discarded.saturating_add(1);
            return None;
        }

        let available_duration_us = samples_to_micros(active.samples.len(), TARGET_SAMPLE_RATE_HZ);
        let available_end = TimestampUs(active.start.0.saturating_add(available_duration_us));
        let end = requested_end.unwrap_or(available_end);
        let end = TimestampUs(end.0.min(available_end.0));
        let wanted_samples =
            micros_to_samples(end.0.saturating_sub(active.start.0), TARGET_SAMPLE_RATE_HZ)
                .min(active.samples.len());
        active.samples.truncate(wanted_samples);
        let exact_end = TimestampUs(active.start.0.saturating_add(samples_to_micros(
            active.samples.len(),
            TARGET_SAMPLE_RATE_HZ,
        )));

        self.metrics.segments_finalized = self.metrics.segments_finalized.saturating_add(1);
        if !split_at_max {
            self.pre_roll.clear();
        }
        Some(SpeechSegment {
            start: active.start,
            end: exact_end,
            samples: active.samples,
        })
    }
}

fn validate_config(config: &SegmenterConfig) -> Result<()> {
    if !config.energy_threshold.is_finite() || config.energy_threshold <= 0.0 {
        return Err(Error::InvalidSegmenterConfig(
            "energy threshold must be finite and greater than zero".to_owned(),
        ));
    }
    if config.start_trigger_ms == 0
        || config.end_silence_ms == 0
        || config.max_segment_ms == 0
        || config.min_speech_ms > config.max_segment_ms
    {
        return Err(Error::InvalidSegmenterConfig(
            "durations must be non-zero and minimum speech cannot exceed maximum segment"
                .to_owned(),
        ));
    }
    Ok(())
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_square =
        samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    mean_square.sqrt()
}

fn total_duration(chunks: &VecDeque<PcmChunk>) -> u64 {
    chunks.iter().map(PcmChunk::duration_us).sum()
}

const fn millis_to_micros(milliseconds: u64) -> u64 {
    milliseconds.saturating_mul(1_000)
}

fn samples_to_micros(samples: usize, sample_rate_hz: u32) -> u64 {
    samples as u64 * 1_000_000 / u64::from(sample_rate_hz)
}

fn micros_to_samples(microseconds: u64, sample_rate_hz: u32) -> usize {
    (u128::from(microseconds) * u128::from(sample_rate_hz) / 1_000_000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AudioSourceKind;

    fn config() -> SegmenterConfig {
        SegmenterConfig {
            energy_threshold: 0.1,
            start_trigger_ms: 40,
            end_silence_ms: 100,
            pre_roll_ms: 40,
            post_roll_ms: 20,
            min_speech_ms: 40,
            max_segment_ms: 400,
        }
    }

    fn chunk(index: u64, level: f32) -> PcmChunk {
        PcmChunk {
            source: AudioSourceKind::File,
            start: TimestampUs(index * 20_000),
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
            samples: vec![level; 320],
        }
    }

    #[test]
    fn short_pause_does_not_split_a_phrase() {
        let mut segmenter = Segmenter::new(config()).expect("valid config");
        let levels = [0.0, 0.5, 0.5, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        let segments = levels
            .into_iter()
            .enumerate()
            .flat_map(|(index, level)| segmenter.push_chunk(chunk(index as u64, level)))
            .collect::<Vec<_>>();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, TimestampUs(0));
        assert_eq!(segments[0].end, TimestampUs(160_000));
    }

    #[test]
    fn long_speech_is_split_without_losing_samples() {
        let mut segmenter = Segmenter::new(config()).expect("valid config");
        let mut segments = (0..42)
            .flat_map(|index| segmenter.push_chunk(chunk(index, 0.5)))
            .collect::<Vec<_>>();
        segments.extend(segmenter.flush());

        assert_eq!(segments.len(), 3);
        assert!(segments.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.samples.len())
                .sum::<usize>(),
            42 * 320
        );
        assert_eq!(segmenter.metrics().max_duration_splits, 2);
    }

    #[test]
    fn flush_emits_pending_speech_and_discards_short_noise() {
        let mut segmenter = Segmenter::new(config()).expect("valid config");
        segmenter.push_chunk(chunk(0, 0.5));
        assert!(segmenter.flush().is_empty());

        segmenter.push_chunk(chunk(1, 0.5));
        segmenter.push_chunk(chunk(2, 0.5));
        let segments = segmenter.flush();
        assert_eq!(segments.len(), 1);
        assert_eq!(segmenter.metrics().segments_discarded, 1);
    }
}
