use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};

use crate::audio::DEFAULT_CHUNK_DURATION_MS;
use crate::domain::{
    AudioSourceKind, CaptureMetrics, PcmChunk, TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ, TimestampUs,
};
use crate::{Error, Result};

const RAW_QUEUE_CAPACITY: usize = 512;
const RAW_RECEIVE_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_NORMALIZED_BUFFER_FRAMES: usize = TARGET_SAMPLE_RATE_HZ as usize * 5;

#[derive(Debug)]
struct RawInput {
    source: AudioSourceKind,
    captured_at_us: u64,
    sample_rate_hz: u32,
    channels: u16,
    samples: Vec<f32>,
}

#[derive(Debug)]
pub struct LiveAudioChunk {
    pub microphone: PcmChunk,
    pub system: PcmChunk,
    pub mixed: PcmChunk,
    pub capture: CaptureMetrics,
}

pub struct LiveAudioConfig {
    microphone_name: String,
    system_name: String,
}

pub struct LiveAudioSource {
    microphone_name: String,
    system_name: String,
    microphone_stream: cpal::Stream,
    system_stream: cpal::Stream,
    raw_receiver: Receiver<RawInput>,
    stream_error_receiver: Receiver<String>,
    dropped_callbacks: Arc<AtomicU64>,
}

pub fn probe(
    microphone_name: Option<&str>,
    system_name: Option<&str>,
    duration: Duration,
) -> Result<CaptureMetrics> {
    let config = LiveAudioConfig::resolve(microphone_name, system_name)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut latest = None;
    std::thread::scope(|scope| -> Result<()> {
        let timer_shutdown = Arc::clone(&shutdown);
        scope.spawn(move || {
            std::thread::sleep(duration);
            timer_shutdown.store(true, Ordering::Relaxed);
        });
        config.open()?.run(&shutdown, |chunk, _finalizing| {
            latest = Some(chunk.capture);
            true
        })
    })?;
    latest.ok_or_else(|| Error::Audio("audio probe received no complete mixed chunk".to_owned()))
}

impl LiveAudioConfig {
    pub fn resolve(microphone_name: Option<&str>, system_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let microphone = match microphone_name {
            Some(name) => find_input_device(&host, name)?,
            None => host.default_input_device().ok_or_else(|| {
                Error::Audio("no default microphone input is available".to_owned())
            })?,
        };
        let microphone_name = device_name(&microphone)?;

        let system = match system_name {
            Some(name) => find_input_device(&host, name)?,
            None => host
                .input_devices()
                .map_err(audio_error)?
                .find(|device| {
                    device
                        .name()
                        .is_ok_and(|name| name.to_ascii_lowercase().contains("blackhole"))
                })
                .ok_or_else(|| {
                    Error::Audio(
                        "no BlackHole input found; pass --system-audio with an exact device name"
                            .to_owned(),
                    )
                })?,
        };
        let system_name = device_name(&system)?;
        if microphone_name == system_name {
            return Err(Error::Audio(
                "microphone and system audio must be different input devices".to_owned(),
            ));
        }

        Ok(Self {
            microphone_name,
            system_name,
        })
    }

    #[must_use]
    pub fn microphone_name(&self) -> &str {
        &self.microphone_name
    }

    #[must_use]
    pub fn system_name(&self) -> &str {
        &self.system_name
    }

    pub fn open(self) -> Result<LiveAudioSource> {
        let host = cpal::default_host();
        let microphone = find_input_device(&host, &self.microphone_name)?;
        let system = find_input_device(&host, &self.system_name)?;
        let (raw_sender, raw_receiver) = bounded(RAW_QUEUE_CAPACITY);
        let (stream_error_sender, stream_error_receiver) = bounded(8);
        let dropped_callbacks = Arc::new(AtomicU64::new(0));
        let capture_epoch = Instant::now();
        let microphone_stream = build_stream(
            &microphone,
            AudioSourceKind::Microphone,
            raw_sender.clone(),
            stream_error_sender.clone(),
            Arc::clone(&dropped_callbacks),
            capture_epoch,
        )?;
        let system_stream = build_stream(
            &system,
            AudioSourceKind::System,
            raw_sender,
            stream_error_sender,
            Arc::clone(&dropped_callbacks),
            capture_epoch,
        )?;

        Ok(LiveAudioSource {
            microphone_name: self.microphone_name,
            system_name: self.system_name,
            microphone_stream,
            system_stream,
            raw_receiver,
            stream_error_receiver,
            dropped_callbacks,
        })
    }
}

impl LiveAudioSource {
    #[must_use]
    pub fn microphone_name(&self) -> &str {
        &self.microphone_name
    }

    #[must_use]
    pub fn system_name(&self) -> &str {
        &self.system_name
    }

    pub fn run(
        self,
        shutdown: &AtomicBool,
        mut on_chunk: impl FnMut(LiveAudioChunk, bool) -> bool,
    ) -> Result<()> {
        self.microphone_stream.play().map_err(audio_error)?;
        self.system_stream.play().map_err(audio_error)?;

        let mut mixer = LiveMixer::new();
        while !shutdown.load(Ordering::Relaxed) {
            if let Ok(error) = self.stream_error_receiver.try_recv() {
                return Err(Error::Audio(error));
            }
            let dropped = self.dropped_callbacks.load(Ordering::Relaxed);
            if dropped > 0 {
                return Err(Error::Audio(format!(
                    "capture callback queue overflowed; {dropped} block(s) were not recorded"
                )));
            }

            match self.raw_receiver.recv_timeout(RAW_RECEIVE_TIMEOUT) {
                Ok(input) => {
                    mixer.push(input)?;
                    while let Some(chunk) = mixer.pop_chunk(dropped) {
                        if !on_chunk(chunk, false) {
                            return Ok(());
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Audio(
                        "both audio capture callbacks disconnected unexpectedly".to_owned(),
                    ));
                }
            }
        }

        while let Ok(input) = self.raw_receiver.try_recv() {
            mixer.push(input)?;
        }
        while let Some(chunk) =
            mixer.pop_padded_chunk(self.dropped_callbacks.load(Ordering::Relaxed))
        {
            if !on_chunk(chunk, true) {
                break;
            }
        }
        Ok(())
    }
}

fn find_input_device(host: &cpal::Host, wanted: &str) -> Result<cpal::Device> {
    let matches = host
        .input_devices()
        .map_err(audio_error)?
        .filter_map(|device| {
            let name = device.name().ok()?;
            (name == wanted).then_some(device)
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one device exists")),
        0 => Err(Error::Audio(format!(
            "input device `{wanted}` was not found; use `devices` for exact names"
        ))),
        count => Err(Error::Audio(format!(
            "input device name `{wanted}` is ambiguous ({count} matches)"
        ))),
    }
}

fn device_name(device: &cpal::Device) -> Result<String> {
    device.name().map_err(audio_error)
}

fn build_stream(
    device: &cpal::Device,
    source: AudioSourceKind,
    raw_sender: Sender<RawInput>,
    stream_error_sender: Sender<String>,
    dropped_callbacks: Arc<AtomicU64>,
    capture_epoch: Instant,
) -> Result<cpal::Stream> {
    let supported = device.default_input_config().map_err(audio_error)?;
    let sample_format = supported.sample_format();
    let config = supported.config();
    match sample_format {
        SampleFormat::F32 => build_typed_stream::<f32>(
            device,
            config,
            source,
            raw_sender,
            stream_error_sender,
            dropped_callbacks,
            capture_epoch,
        ),
        SampleFormat::I16 => build_typed_stream::<i16>(
            device,
            config,
            source,
            raw_sender,
            stream_error_sender,
            dropped_callbacks,
            capture_epoch,
        ),
        SampleFormat::U16 => build_typed_stream::<u16>(
            device,
            config,
            source,
            raw_sender,
            stream_error_sender,
            dropped_callbacks,
            capture_epoch,
        ),
        other => Err(Error::Audio(format!(
            "input sample format {other} is not supported; supported formats are f32, i16 and u16"
        ))),
    }
}

fn build_typed_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    source: AudioSourceKind,
    raw_sender: Sender<RawInput>,
    stream_error_sender: Sender<String>,
    dropped_callbacks: Arc<AtomicU64>,
    capture_epoch: Instant,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let sample_rate_hz = config.sample_rate.0;
    let channels = config.channels;
    device
        .build_input_stream::<T, _, _>(
            &config,
            move |data, _| {
                let samples = data.iter().copied().map(Sample::to_sample::<f32>).collect();
                if raw_sender
                    .try_send(RawInput {
                        source,
                        captured_at_us: capture_epoch
                            .elapsed()
                            .as_micros()
                            .try_into()
                            .unwrap_or(u64::MAX),
                        sample_rate_hz,
                        channels,
                        samples,
                    })
                    .is_err()
                {
                    dropped_callbacks.fetch_add(1, Ordering::Relaxed);
                }
            },
            move |error| {
                let _ = stream_error_sender.try_send(format!("{source:?} stream failed: {error}"));
            },
            None,
        )
        .map_err(audio_error)
}

fn audio_error(error: impl std::fmt::Display) -> Error {
    Error::Audio(error.to_string())
}

struct Normalizer {
    source_rate: u32,
    phase: f64,
    previous: Option<f32>,
}

impl Normalizer {
    fn new(source_rate: u32) -> Self {
        Self {
            source_rate,
            phase: 0.0,
            previous: None,
        }
    }

    fn push(&mut self, interleaved: &[f32], channels: u16) -> Vec<f32> {
        if channels == 0 || self.source_rate == 0 {
            return Vec::new();
        }
        let channels = usize::from(channels);
        let mono = interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect::<Vec<_>>();
        if mono.is_empty() {
            return Vec::new();
        }

        let mut input = Vec::with_capacity(mono.len() + usize::from(self.previous.is_some()));
        if let Some(previous) = self.previous {
            input.push(previous);
        }
        input.extend_from_slice(&mono);
        self.previous = mono.last().copied();

        let step = f64::from(self.source_rate) / f64::from(TARGET_SAMPLE_RATE_HZ);
        let mut output = Vec::new();
        while self.phase + 1.0 < input.len() as f64 {
            let lower = self.phase.floor() as usize;
            let fraction = (self.phase - lower as f64) as f32;
            output.push(input[lower] + (input[lower + 1] - input[lower]) * fraction);
            self.phase += step;
        }
        self.phase -= (input.len() - 1) as f64;
        output
    }
}

struct LiveMixer {
    microphone: VecDeque<f32>,
    system: VecDeque<f32>,
    microphone_normalizer: Option<Normalizer>,
    system_normalizer: Option<Normalizer>,
    frames_emitted: u64,
    microphone_frames: u64,
    system_frames: u64,
    microphone_peak: f32,
    system_peak: f32,
    microphone_started_at_us: Option<u64>,
    system_started_at_us: Option<u64>,
    start_alignment_applied: bool,
    started: Instant,
}

impl LiveMixer {
    fn new() -> Self {
        Self {
            microphone: VecDeque::new(),
            system: VecDeque::new(),
            microphone_normalizer: None,
            system_normalizer: None,
            frames_emitted: 0,
            microphone_frames: 0,
            system_frames: 0,
            microphone_peak: 0.0,
            system_peak: 0.0,
            microphone_started_at_us: None,
            system_started_at_us: None,
            start_alignment_applied: false,
            started: Instant::now(),
        }
    }

    fn push(&mut self, input: RawInput) -> Result<()> {
        match input.source {
            AudioSourceKind::Microphone => {
                self.microphone_started_at_us
                    .get_or_insert(input.captured_at_us);
            }
            AudioSourceKind::System => {
                self.system_started_at_us
                    .get_or_insert(input.captured_at_us);
            }
            _ => {}
        }
        let (normalizer, queue, total, peak) = match input.source {
            AudioSourceKind::Microphone => (
                &mut self.microphone_normalizer,
                &mut self.microphone,
                &mut self.microphone_frames,
                &mut self.microphone_peak,
            ),
            AudioSourceKind::System => (
                &mut self.system_normalizer,
                &mut self.system,
                &mut self.system_frames,
                &mut self.system_peak,
            ),
            _ => return Ok(()),
        };
        let normalizer = normalizer.get_or_insert_with(|| Normalizer::new(input.sample_rate_hz));
        if normalizer.source_rate != input.sample_rate_hz {
            return Err(Error::Audio(format!(
                "{:?} sample rate changed from {} Hz to {} Hz during capture",
                input.source, normalizer.source_rate, input.sample_rate_hz
            )));
        }
        let normalized = normalizer.push(&input.samples, input.channels);
        *peak = peak.max(
            normalized
                .iter()
                .fold(0.0_f32, |maximum, sample| maximum.max(sample.abs())),
        );
        *total = total.saturating_add(normalized.len() as u64);
        queue.extend(normalized);
        if queue.len() > MAX_NORMALIZED_BUFFER_FRAMES {
            return Err(Error::Audio(format!(
                "{:?} exceeded five seconds of unmatched buffered audio; the other input likely stalled",
                input.source
            )));
        }
        self.apply_start_alignment()?;
        Ok(())
    }

    fn apply_start_alignment(&mut self) -> Result<()> {
        if self.start_alignment_applied {
            return Ok(());
        }
        let (Some(microphone_start), Some(system_start)) =
            (self.microphone_started_at_us, self.system_started_at_us)
        else {
            return Ok(());
        };
        let offset_frames = (microphone_start.abs_diff(system_start) as u128
            * u128::from(TARGET_SAMPLE_RATE_HZ)
            / 1_000_000)
            .min(MAX_NORMALIZED_BUFFER_FRAMES as u128) as usize;
        let delayed_queue = if microphone_start > system_start {
            &mut self.microphone
        } else {
            &mut self.system
        };
        if delayed_queue.len().saturating_add(offset_frames) > MAX_NORMALIZED_BUFFER_FRAMES {
            return Err(Error::Audio(
                "live inputs started more than five seconds apart".to_owned(),
            ));
        }
        prepend_silence(delayed_queue, offset_frames);
        self.start_alignment_applied = true;
        Ok(())
    }

    fn pop_chunk(&mut self, dropped_callbacks: u64) -> Option<LiveAudioChunk> {
        let chunk_frames =
            (u64::from(TARGET_SAMPLE_RATE_HZ) * DEFAULT_CHUNK_DURATION_MS / 1_000) as usize;
        if self.microphone.len() < chunk_frames || self.system.len() < chunk_frames {
            return None;
        }
        Some(self.take_chunk(chunk_frames, false, dropped_callbacks))
    }

    fn pop_padded_chunk(&mut self, dropped_callbacks: u64) -> Option<LiveAudioChunk> {
        if self.microphone.is_empty() && self.system.is_empty() {
            return None;
        }
        let chunk_frames =
            (u64::from(TARGET_SAMPLE_RATE_HZ) * DEFAULT_CHUNK_DURATION_MS / 1_000) as usize;
        let frames = self
            .microphone
            .len()
            .max(self.system.len())
            .min(chunk_frames);
        Some(self.take_chunk(frames, true, dropped_callbacks))
    }

    fn take_chunk(
        &mut self,
        frames: usize,
        pad_missing: bool,
        dropped_callbacks: u64,
    ) -> LiveAudioChunk {
        let microphone = drain_samples(&mut self.microphone, frames, pad_missing);
        let system = drain_samples(&mut self.system, frames, pad_missing);
        let mixed = microphone
            .iter()
            .zip(&system)
            .map(|(microphone, system)| ((microphone + system) * 0.5).clamp(-1.0, 1.0))
            .collect::<Vec<_>>();
        let start = TimestampUs(
            self.frames_emitted.saturating_mul(1_000_000) / u64::from(TARGET_SAMPLE_RATE_HZ),
        );
        self.frames_emitted = self.frames_emitted.saturating_add(frames as u64);
        let drift_frames = self.microphone_frames as i128 - self.system_frames as i128;
        let drift_us = (drift_frames * 1_000_000 / i128::from(TARGET_SAMPLE_RATE_HZ))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        let capture = CaptureMetrics {
            microphone_peak_milli: amplitude_milli(self.microphone_peak),
            system_peak_milli: amplitude_milli(self.system_peak),
            microphone_signal: self.microphone_peak >= 0.001,
            system_signal: self.system_peak >= 0.001,
            clock_drift_us: drift_us,
            callback_blocks_dropped: dropped_callbacks,
            elapsed_us: self
                .started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        };

        LiveAudioChunk {
            microphone: pcm(AudioSourceKind::Microphone, start, microphone),
            system: pcm(AudioSourceKind::System, start, system),
            mixed: pcm(AudioSourceKind::Mixed, start, mixed),
            capture,
        }
    }
}

fn drain_samples(queue: &mut VecDeque<f32>, frames: usize, pad_missing: bool) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames);
    for _ in 0..frames {
        match queue.pop_front() {
            Some(sample) => samples.push(sample),
            None if pad_missing => samples.push(0.0),
            None => break,
        }
    }
    samples
}

fn prepend_silence(queue: &mut VecDeque<f32>, frames: usize) {
    for _ in 0..frames {
        queue.push_front(0.0);
    }
}

fn pcm(source: AudioSourceKind, start: TimestampUs, samples: Vec<f32>) -> PcmChunk {
    PcmChunk {
        source,
        start,
        sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
        channels: TARGET_CHANNELS,
        samples,
    }
}

fn amplitude_milli(amplitude: f32) -> u32 {
    (amplitude.clamp(0.0, 1.0) * 1_000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_attenuates_sources_and_reports_signal_and_drift() {
        let mut mixer = LiveMixer::new();
        mixer
            .push(raw(AudioSourceKind::Microphone, vec![1.0; 321]))
            .expect("microphone input should normalize");
        mixer
            .push(raw(AudioSourceKind::System, vec![0.5; 321]))
            .expect("system input should normalize");

        let chunk = mixer.pop_chunk(0).expect("a complete chunk should mix");

        assert_eq!(chunk.mixed.samples.len(), 320);
        assert!(chunk.mixed.samples.iter().all(|sample| *sample == 0.75));
        assert!(chunk.capture.microphone_signal);
        assert!(chunk.capture.system_signal);
        assert_eq!(chunk.capture.clock_drift_us, 0);
        assert_eq!(chunk.capture.callback_blocks_dropped, 0);
    }

    #[test]
    fn final_chunk_pads_shorter_source_without_losing_the_longer_tail() {
        let mut mixer = LiveMixer::new();
        mixer
            .push(raw(AudioSourceKind::Microphone, vec![1.0; 101]))
            .expect("microphone input should normalize");
        mixer
            .push(raw(AudioSourceKind::System, vec![0.0; 51]))
            .expect("system input should normalize");

        let chunk = mixer
            .pop_padded_chunk(0)
            .expect("remaining source audio should flush");

        assert_eq!(chunk.microphone.samples.len(), 100);
        assert_eq!(chunk.system.samples.len(), 100);
        assert_eq!(chunk.mixed.samples[75], 0.5);
    }

    #[test]
    fn streaming_resampler_keeps_duration_across_callback_boundaries() {
        let mut normalizer = Normalizer::new(48_000);
        let first = normalizer.push(&vec![0.25; 480], 1);
        let second = normalizer.push(&vec![0.25; 480], 1);

        assert!((first.len() + second.len()).abs_diff(320) <= 1);
        assert!(first.iter().chain(&second).all(|sample| *sample == 0.25));
    }

    #[test]
    fn mixer_rejects_unbounded_growth_when_the_other_source_stalls() {
        let mut mixer = LiveMixer::new();
        let error = mixer
            .push(raw(
                AudioSourceKind::Microphone,
                vec![0.1; MAX_NORMALIZED_BUFFER_FRAMES + 2],
            ))
            .expect_err("an unmatched source must remain bounded");

        assert!(error.to_string().contains("other input likely stalled"));
    }

    #[test]
    fn mixer_rejects_a_sample_rate_change_mid_stream() {
        let mut mixer = LiveMixer::new();
        mixer
            .push(raw(AudioSourceKind::System, vec![0.1; 100]))
            .expect("first format should be accepted");
        let mut changed = raw(AudioSourceKind::System, vec![0.1; 100]);
        changed.sample_rate_hz = 48_000;

        assert!(
            mixer
                .push(changed)
                .expect_err("format changes must be explicit failures")
                .to_string()
                .contains("sample rate changed")
        );
    }

    #[test]
    fn mixer_aligns_different_callback_start_times_with_silence() {
        let mut mixer = LiveMixer::new();
        let mut microphone = raw(AudioSourceKind::Microphone, vec![1.0; 641]);
        microphone.captured_at_us = 0;
        let mut system = raw(AudioSourceKind::System, vec![1.0; 321]);
        system.captured_at_us = 20_000;
        mixer
            .push(microphone)
            .expect("microphone input should normalize");
        mixer.push(system).expect("system input should normalize");

        let first = mixer.pop_chunk(0).expect("aligned chunk should exist");
        assert!(first.system.samples.iter().all(|sample| *sample == 0.0));
        assert!(first.mixed.samples.iter().all(|sample| *sample == 0.5));
    }

    fn raw(source: AudioSourceKind, samples: Vec<f32>) -> RawInput {
        RawInput {
            source,
            captured_at_us: 0,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: 1,
            samples,
        }
    }
}
