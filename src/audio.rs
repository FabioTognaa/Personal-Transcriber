use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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

    /// One chunk by index, used by the one-shot `transcribe` path which pulls
    /// chunks instead of iterating. [`FileAudioSource::chunks`] is left untouched
    /// for the live-simulator callers.
    pub(crate) fn chunk_at(&self, index: usize) -> Option<PcmChunk> {
        let start = index.checked_mul(self.chunk_frames)?;
        let end = start
            .saturating_add(self.chunk_frames)
            .min(self.samples.len());
        let samples = self.samples.get(start..end)?;
        if samples.is_empty() {
            return None;
        }
        Some(PcmChunk {
            source: AudioSourceKind::File,
            start: TimestampUs(
                (start as u64).saturating_mul(1_000_000) / u64::from(TARGET_SAMPLE_RATE_HZ),
            ),
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
            samples: samples.to_vec(),
        })
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

/// Decodes a compressed audio file (MP3, M4A/MP4-AAC, AAC) packet by packet and
/// emits canonical 16 kHz mono chunks. Memory stays bounded regardless of file
/// duration because nothing but the current packet and a partial chunk is held.
pub struct CompressedAudioSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    normalizer: Option<Normalizer>,
    output: Vec<f32>,
    chunk_frames: usize,
    emitted_samples: u64,
    eof: bool,
    /// Total source duration when the container declares it, so callers can show
    /// progress. `None` means the format does not say (see `total_duration_us`).
    total_us: Option<u64>,
}

impl CompressedAudioSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(std::ffi::OsStr::to_str) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|error| Error::UnsupportedAudio(error.to_string()))?;
        let format = probed.format;
        let (track_id, decoder) = select_decoder(&*format)?;
        let total_us = total_duration_us(&*format, track_id);
        let chunk_frames =
            (u64::from(TARGET_SAMPLE_RATE_HZ) * DEFAULT_CHUNK_DURATION_MS / 1_000) as usize;
        Ok(Self {
            reader: format,
            decoder,
            track_id,
            normalizer: None,
            output: Vec::new(),
            chunk_frames,
            emitted_samples: 0,
            eof: false,
            total_us,
        })
    }

    /// Total source duration in microseconds, when the container declares it.
    ///
    /// ponytail: derived from the track header. For MP3 without a Xing/Info header
    /// and for raw ADTS AAC this is an estimate (symphonia assumes CBR); a VBR file
    /// without a header can drift by a few percent. If a container ever reports
    /// nothing, callers fall back to an indeterminate progress line rather than
    /// scanning the file up front.
    pub fn total_duration_us(&self) -> Option<u64> {
        self.total_us
    }

    pub fn next_chunk(&mut self) -> Result<Option<PcmChunk>> {
        loop {
            if self.output.len() >= self.chunk_frames {
                return Ok(Some(self.emit(self.chunk_frames)));
            }
            if self.eof {
                if let Some(normalizer) = self.normalizer.as_mut() {
                    normalizer.flush(&mut self.output);
                }
                if self.output.is_empty() {
                    return Ok(None);
                }
                let remaining = self.output.len();
                return Ok(Some(self.emit(remaining)));
            }
            match self.reader.next_packet() {
                Ok(packet) => {
                    if packet.track_id() != self.track_id {
                        continue;
                    }
                    consume_packet(
                        &mut self.decoder,
                        &packet,
                        &mut self.normalizer,
                        &mut self.output,
                    )?;
                }
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                }
                Err(SymphoniaError::ResetRequired) => self.eof = true,
                Err(error) => return Err(Error::UnsupportedAudio(error.to_string())),
            }
        }
    }

    fn emit(&mut self, len: usize) -> PcmChunk {
        let samples: Vec<f32> = self.output.drain(..len).collect();
        let start = TimestampUs(
            self.emitted_samples.saturating_mul(1_000_000) / u64::from(TARGET_SAMPLE_RATE_HZ),
        );
        self.emitted_samples = self.emitted_samples.saturating_add(len as u64);
        PcmChunk {
            source: AudioSourceKind::File,
            start,
            sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS,
            samples,
        }
    }
}

/// Total duration in microseconds from the track header: `n_frames` at the source
/// sample rate. `None` when the container does not declare either.
fn total_duration_us(reader: &dyn FormatReader, track_id: u32) -> Option<u64> {
    let track = reader.tracks().iter().find(|track| track.id == track_id)?;
    match (track.codec_params.n_frames, track.codec_params.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => {
            Some(frames.saturating_mul(1_000_000) / u64::from(rate))
        }
        _ => None,
    }
}

/// Pick the first track the codec registry can actually decode, so an MP4 with a
/// video track first still resolves to its audio track.
fn select_decoder(reader: &dyn FormatReader) -> Result<(u32, Box<dyn Decoder>)> {
    for track in reader.tracks() {
        if track.codec_params.codec == CODEC_TYPE_NULL {
            continue;
        }
        if let Ok(decoder) =
            symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        {
            return Ok((track.id, decoder));
        }
    }
    Err(Error::UnsupportedAudio(
        "no decodable audio track found".to_owned(),
    ))
}

fn consume_packet(
    decoder: &mut Box<dyn Decoder>,
    packet: &Packet,
    normalizer: &mut Option<Normalizer>,
    output: &mut Vec<f32>,
) -> Result<()> {
    match decoder.decode(packet) {
        Ok(decoded) => {
            let spec = *decoded.spec();
            let mut sample_buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            sample_buffer.copy_interleaved_ref(decoded);
            let channels = spec.channels.count().max(1);
            let normalizer = normalizer.get_or_insert_with(|| Normalizer::new(channels, spec.rate));
            normalizer.push_interleaved(sample_buffer.samples(), channels, output);
            Ok(())
        }
        // Corrupt or unreadable packets are skipped, matching the reference
        // decode loop: a single bad frame must not abort the whole file.
        Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => Ok(()),
        Err(error) => Err(Error::UnsupportedAudio(error.to_string())),
    }
}

/// Streams interleaved PCM into 16 kHz mono by downmixing each packet and
/// resampling with a cursor that carries across packet boundaries, so no click
/// appears at a packet seam.
pub(crate) struct Normalizer {
    channels: usize,
    source_rate: u32,
    cursor: f64,
    mono: Vec<f32>,
}

impl Normalizer {
    pub(crate) fn new(channels: usize, source_rate: u32) -> Self {
        Self {
            channels,
            source_rate,
            cursor: 0.0,
            mono: Vec::new(),
        }
    }

    pub(crate) fn source_rate(&self) -> u32 {
        self.source_rate
    }

    pub(crate) fn push_interleaved(
        &mut self,
        interleaved: &[f32],
        channels: usize,
        output: &mut Vec<f32>,
    ) {
        self.channels = channels.max(1);
        for frame in interleaved.chunks_exact(self.channels) {
            self.mono
                .push(frame.iter().sum::<f32>() / self.channels as f32);
        }
        self.resample_into(output);
    }

    fn resample_into(&mut self, output: &mut Vec<f32>) {
        if self.source_rate == 0 {
            return;
        }
        if self.source_rate == TARGET_SAMPLE_RATE_HZ {
            output.append(&mut self.mono);
            return;
        }
        let ratio = f64::from(self.source_rate) / f64::from(TARGET_SAMPLE_RATE_HZ);
        while (self.cursor.floor() as usize) + 1 < self.mono.len() {
            let lower = self.cursor.floor() as usize;
            let fraction = (self.cursor - lower as f64) as f32;
            output.push(lerp(self.mono[lower], self.mono[lower + 1], fraction));
            self.cursor += ratio;
        }
        let consumed = (self.cursor.floor() as usize).min(self.mono.len());
        if consumed > 0 {
            self.mono.drain(..consumed);
            self.cursor -= consumed as f64;
        }
    }

    fn flush(&mut self, output: &mut Vec<f32>) {
        if self.source_rate == 0 {
            return;
        }
        if self.source_rate == TARGET_SAMPLE_RATE_HZ {
            output.append(&mut self.mono);
            return;
        }
        if self.mono.is_empty() {
            return;
        }
        let ratio = f64::from(self.source_rate) / f64::from(TARGET_SAMPLE_RATE_HZ);
        while (self.cursor.floor() as usize) < self.mono.len() {
            let lower = self.cursor.floor() as usize;
            let upper = (lower + 1).min(self.mono.len() - 1);
            let fraction = (self.cursor - lower as f64) as f32;
            output.push(lerp(self.mono[lower], self.mono[upper], fraction));
            self.cursor += ratio;
        }
        self.mono.clear();
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
    fn streaming_resampler_matches_the_batch_resampler() {
        let input: Vec<f32> = (0..1_000)
            .map(|index| (index as f32 * 0.01).sin())
            .collect();
        let expected = resample_linear(&input, 8_000, 16_000);

        let mut normalizer = Normalizer::new(1, 8_000);
        let mut output = Vec::new();
        for packet in input.chunks(137) {
            normalizer.push_interleaved(packet, 1, &mut output);
        }
        normalizer.flush(&mut output);

        assert_eq!(output.len(), expected.len());
        let max_diff = output
            .iter()
            .zip(&expected)
            .map(|(produced, reference)| (produced - reference).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_diff < 1e-4, "packet seams introduced {max_diff}");
    }

    #[test]
    fn compressed_sources_decode_to_16khz_mono() {
        // 1 s 440 Hz stereo 44.1 kHz, generated with ffmpeg and versioned.
        for name in ["tone_1s.mp3", "tone_1s.m4a"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name);
            let mut source = CompressedAudioSource::open(&path).expect("source should open");
            let mut total = 0_usize;
            while let Some(chunk) = source.next_chunk().expect("chunk should decode") {
                assert_eq!(chunk.channels, TARGET_CHANNELS);
                assert_eq!(chunk.sample_rate_hz, TARGET_SAMPLE_RATE_HZ);
                total += chunk.samples.len();
            }
            let difference = (total as i64 - 16_000).unsigned_abs();
            assert!(difference < 3_200, "{name} decoded to {total} samples");
        }
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
