use std::collections::VecDeque;

use crate::domain::{
    PcmChunk, SegmentationMetrics, SegmenterConfig, SpeechSegment, TARGET_CHANNELS,
    TARGET_SAMPLE_RATE_HZ, TimestampUs,
};
use crate::{Error, Result};

/// Silence runs shorter than this are not split candidates: they are energy
/// dips inside a word, not pauses between words.
const SPLIT_SILENCE_MIN_MS: u64 = 80;
/// Once speech is open, the voice threshold drops by this factor so chunks at
/// the speech/silence boundary do not flicker the segment open and closed.
const HYSTERESIS_RATIO: f32 = 2.0;
/// Safety ceiling for the adaptive threshold; a background noise this loud
/// makes the recording unusable anyway.
const VAD_THRESHOLD_MAX: f32 = 0.25;
/// Time constant of the background-noise estimate.
const NOISE_TAU_SECONDS: f32 = 1.0;

/// A maximal run of unvoiced chunks inside an active segment, kept to find a
/// pause to split long segments on.
#[derive(Debug)]
struct SilenceRun {
    /// Sample offset of the run start within the active segment.
    start: usize,
    /// Sample offset of the run end (exclusive) within the active segment.
    end: usize,
    /// Speech duration accumulated before the run started.
    speech_before_us: u64,
}

#[derive(Debug)]
struct ActiveSegment {
    start: TimestampUs,
    samples: Vec<f32>,
    speech_duration_us: u64,
    last_voice_end: TimestampUs,
    trailing_silence_us: u64,
    silence_runs: Vec<SilenceRun>,
}

#[derive(Debug)]
pub struct Segmenter {
    config: SegmenterConfig,
    pre_roll: VecDeque<PcmChunk>,
    active: Option<ActiveSegment>,
    voice_run_us: u64,
    first_voice_start: Option<TimestampUs>,
    /// Exponential moving average of the RMS of non-voice chunks: the
    /// estimated background noise the voice threshold adapts to.
    noise_floor: f32,
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
            noise_floor: 0.0,
            metrics: SegmentationMetrics::default(),
        })
    }

    #[must_use]
    pub fn metrics(&self) -> SegmentationMetrics {
        self.metrics
    }

    /// Threshold to open speech: the estimated background noise raised by the
    /// configured multiplier, never below the manual floor, never above the
    /// safety ceiling.
    fn open_threshold(&self) -> f32 {
        let adaptive = (self.noise_floor * self.config.noise_multiplier).min(VAD_THRESHOLD_MAX);
        adaptive.max(self.config.energy_threshold)
    }

    /// Threshold to keep speech open: lower than the opening one (hysteresis),
    /// but never below the estimated noise itself.
    fn close_threshold(&self, open: f32) -> f32 {
        (open / HYSTERESIS_RATIO).max(self.noise_floor)
    }

    fn update_noise_floor(&mut self, chunk_rms: f32, duration_us: u64) {
        let alpha = (duration_us as f32 / 1_000_000.0 / NOISE_TAU_SECONDS).clamp(0.0, 1.0);
        self.noise_floor += (chunk_rms - self.noise_floor) * alpha;
    }

    pub fn push_chunk(&mut self, chunk: PcmChunk) -> Vec<SpeechSegment> {
        debug_assert_eq!(chunk.sample_rate_hz, TARGET_SAMPLE_RATE_HZ);
        debug_assert_eq!(chunk.channels, TARGET_CHANNELS);

        let duration_us = chunk.duration_us();
        let chunk_rms = rms(&chunk.samples);
        let in_speech = self.active.is_some() || self.voice_run_us > 0;
        let voiced = if in_speech {
            chunk_rms >= self.close_threshold(self.open_threshold())
        } else {
            let open = self.open_threshold();
            let voiced = chunk_rms >= open;
            if !voiced {
                self.update_noise_floor(chunk_rms, duration_us);
            }
            voiced
        };
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
        {
            let active = self.active.as_mut().expect("active segment exists");
            let run_start = active.samples.len();
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
                match active.silence_runs.last_mut() {
                    Some(run) if run.end == run_start => run.end = active.samples.len(),
                    _ => active.silence_runs.push(SilenceRun {
                        start: run_start,
                        end: active.samples.len(),
                        speech_before_us: active.speech_duration_us,
                    }),
                }
            }
        }

        let (segment_duration_us, trailing_silence_us, last_voice_end) = {
            let active = self.active.as_ref().expect("active segment exists");
            (
                chunk_end.0.saturating_sub(active.start.0),
                active.trailing_silence_us,
                active.last_voice_end,
            )
        };
        if segment_duration_us >= millis_to_micros(self.config.max_segment_ms) {
            self.metrics.max_duration_splits = self.metrics.max_duration_splits.saturating_add(1);
            return self.split_at_max();
        }

        if trailing_silence_us >= millis_to_micros(self.config.end_silence_ms) {
            let desired_end = TimestampUs(
                last_voice_end
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

    /// Split an over-long segment on its longest internal pause so the cut
    /// lands between words instead of mid-word. Both halves are guaranteed at
    /// least `min_speech_ms` of speech, so neither is discarded and no samples
    /// are lost; the remainder stays open as the new active segment. Without a
    /// usable pause the segment is cut at the ceiling, like the old rigid split.
    fn split_at_max(&mut self) -> Vec<SpeechSegment> {
        let min_speech_us = millis_to_micros(self.config.min_speech_ms);
        let candidate = self.active.as_ref().and_then(|active| {
            active
                .silence_runs
                .iter()
                .filter(|run| {
                    samples_to_micros(run.end - run.start, TARGET_SAMPLE_RATE_HZ)
                        >= millis_to_micros(SPLIT_SILENCE_MIN_MS)
                        && run.speech_before_us >= min_speech_us
                        && active.speech_duration_us - run.speech_before_us >= min_speech_us
                })
                .max_by_key(|run| run.end - run.start)
                .map(|run| (run.end, run.speech_before_us))
        });

        let Some((cut, speech_before_us)) = candidate else {
            return self.finalize_active(None, true).into_iter().collect();
        };

        let mut active = self.active.take().expect("active segment exists");
        let first_start = active.start;
        let cut_us = first_start
            .0
            .saturating_add(samples_to_micros(cut, TARGET_SAMPLE_RATE_HZ));
        let remainder_last_voice_us = active.last_voice_end.0;
        let remainder_speech_us = active.speech_duration_us - speech_before_us;
        let remainder_runs = active
            .silence_runs
            .iter()
            .filter(|run| run.start >= cut)
            .map(|run| SilenceRun {
                start: run.start - cut,
                end: run.end - cut,
                speech_before_us: run.speech_before_us - speech_before_us,
            })
            .collect::<Vec<_>>();
        let remainder_samples = active.samples.split_off(cut);
        let remainder_end_us =
            cut_us + samples_to_micros(remainder_samples.len(), TARGET_SAMPLE_RATE_HZ);

        self.active = Some(ActiveSegment {
            start: TimestampUs(cut_us),
            samples: remainder_samples,
            speech_duration_us: remainder_speech_us,
            last_voice_end: TimestampUs(remainder_last_voice_us),
            trailing_silence_us: remainder_end_us.saturating_sub(remainder_last_voice_us),
            silence_runs: remainder_runs,
        });
        self.metrics.segments_finalized = self.metrics.segments_finalized.saturating_add(1);
        vec![SpeechSegment {
            start: first_start,
            end: TimestampUs(cut_us),
            samples: active.samples,
        }]
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
            silence_runs: Vec::new(),
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
    if !config.noise_multiplier.is_finite() || config.noise_multiplier <= 0.0 {
        return Err(Error::InvalidSegmenterConfig(
            "noise multiplier must be finite and greater than zero".to_owned(),
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

pub(crate) fn rms(samples: &[f32]) -> f32 {
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
            noise_multiplier: 4.0,
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

    #[test]
    fn a_breath_length_pause_no_longer_splits_an_utterance() {
        // 1.5 s pause inside an utterance: below the 1600 ms end-silence the
        // phrase stays one segment, and the next phrase joins it only if the
        // pause never reaches end_silence_ms.
        let mut config = config();
        config.end_silence_ms = 1600;
        config.max_segment_ms = 4_000;
        let mut segmenter = Segmenter::new(config).expect("valid config");
        let levels = [
            (0.5, 5),  // 100 ms voice
            (0.0, 75), // 1.5 s breath
            (0.5, 5),  // 100 ms voice
        ]
        .into_iter()
        .flat_map(|(level, count)| std::iter::repeat_n(level, count))
        .enumerate()
        .flat_map(|(index, level)| segmenter.push_chunk(chunk(index as u64, level)))
        .collect::<Vec<_>>();

        assert!(levels.is_empty(), "the pause must not close the segment");
        let flushed = segmenter.flush();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].end, TimestampUs(1_700_000));
    }

    #[test]
    fn long_speech_is_split_on_the_longest_pause_not_at_the_ceiling() {
        let mut config = config();
        config.min_speech_ms = 40;
        let mut segmenter = Segmenter::new(config).expect("valid config");
        // 100 ms voice, 80 ms pause (>= the 80 ms split minimum, speech on both
        // sides, below the 100 ms end-silence), then continuous voice well past
        // the 400 ms ceiling: the first split must land at the pause (180 ms),
        // not at 400 ms.
        let schedule: Vec<f32> = std::iter::repeat_n(0.5, 5)
            .chain(std::iter::repeat_n(0.0, 4))
            .chain(std::iter::repeat_n(0.5, 46))
            .collect();
        let mut segments = schedule
            .into_iter()
            .enumerate()
            .flat_map(|(index, level)| segmenter.push_chunk(chunk(index as u64, level)))
            .collect::<Vec<_>>();
        segments.extend(segmenter.flush());

        assert_eq!(segments[0].end, TimestampUs(180_000));
        assert!(segments.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.samples.len())
                .sum::<usize>(),
            55 * 320,
            "no samples may be lost at the split"
        );
    }

    #[test]
    fn segmentation_is_invariant_to_the_overall_input_level() {
        fn boundaries(scale: f32) -> Vec<(TimestampUs, TimestampUs)> {
            let mut segmenter = Segmenter::new(SegmenterConfig::default()).expect("valid config");
            let levels: Vec<f32> = [80, 60, 80, 60, 80]
                .into_iter()
                .flat_map(|count| {
                    let level = if count == 60 { 0.30 } else { 0.004 };
                    std::iter::repeat_n(level * scale, count)
                })
                .collect();
            let mut segments = levels
                .into_iter()
                .enumerate()
                .flat_map(|(index, level)| segmenter.push_chunk(chunk(index as u64, level)))
                .collect::<Vec<_>>();
            segments.extend(segmenter.flush());
            segments
                .into_iter()
                .map(|segment| (segment.start, segment.end))
                .collect()
        }

        // Quiet voice over a loud floor and loud voice over a quiet floor must
        // produce the same boundaries: the adaptive threshold tracks the level.
        assert_eq!(boundaries(1.0), boundaries(0.5));
        assert_eq!(boundaries(1.0).len(), 2);
    }

    #[test]
    fn hysteresis_keeps_speech_open_through_sub_threshold_dips() {
        let mut config = config();
        config.max_segment_ms = 4_000;
        let mut segmenter = Segmenter::new(config).expect("valid config");
        // Voice opens at 0.1; once open the bar drops to 0.05, so the 0.07 dips
        // stay voiced. With a single fixed threshold they would be 100 ms of
        // silence and split the utterance in two.
        let levels: Vec<f32> = std::iter::repeat_n(0.001, 3)
            .chain(std::iter::repeat_n(0.5, 5))
            .chain(std::iter::repeat_n(0.07, 5))
            .chain(std::iter::repeat_n(0.5, 5))
            .chain(std::iter::repeat_n(0.001, 6))
            .collect();
        let mut segments = levels
            .into_iter()
            .enumerate()
            .flat_map(|(index, level)| segmenter.push_chunk(chunk(index as u64, level)))
            .collect::<Vec<_>>();
        segments.extend(segmenter.flush());

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, TimestampUs(20_000));
        assert_eq!(segments[0].end, TimestampUs(380_000));
    }
}
