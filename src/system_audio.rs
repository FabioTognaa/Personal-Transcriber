//! Native macOS system audio capture through Core Audio process taps.
//!
//! A tap mirrors what other processes play, so no virtual audio driver or
//! Multi-Output Device is involved. The tap is exposed as an input stream
//! through a private aggregate device; this module turns that stream into the
//! same [`RawInput`] blocks the cpal path produces.
//!
//! Requires macOS 14.2+. The first capture asks the user for the System Audio
//! Recording permission; without it the tap starts but delivers silence.

use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::Sender;

use crate::domain::AudioSourceKind;
use crate::live_audio::RawInput;
use crate::{Error, Result};

pub(crate) const LABEL: &str = "System Audio (Core Audio tap)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permission {
    Authorized,
    Denied,
    NotDetermined,
    /// The private TCC preflight SPI is unavailable; do not block capture.
    Unknown,
}

struct TapContext {
    sender: Sender<RawInput>,
    epoch: Instant,
    dropped: Arc<AtomicU64>,
}

pub(crate) struct SystemAudioTap {
    handle: *mut std::ffi::c_void,
    context: *mut TapContext,
}

impl SystemAudioTap {
    pub(crate) fn is_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: the shim only checks the OS availability of the tap API.
            unsafe { imp::available() != 0 }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub(crate) fn permission() -> Permission {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: the shim loads TCCAccessPreflight and only reads a grant.
            match unsafe { imp::permission() } {
                0 => Permission::Authorized,
                1 => Permission::Denied,
                2 => Permission::NotDetermined,
                _ => Permission::Unknown,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Permission::Unknown
        }
    }

    pub(crate) fn create(
        sender: Sender<RawInput>,
        epoch: Instant,
        dropped: Arc<AtomicU64>,
    ) -> Result<Self> {
        let context = Box::into_raw(Box::new(TapContext {
            sender,
            epoch,
            dropped,
        }));
        let mut error = [0_i8; 256];

        #[cfg(target_os = "macos")]
        // SAFETY: `context` stays alive until `stop` has run in `Drop`.
        let handle = unsafe {
            imp::create(
                imp::CALLBACK,
                context.cast(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let handle: *mut std::ffi::c_void = std::ptr::null_mut();

        if handle.is_null() {
            // SAFETY: the shim did not take ownership of a failed context.
            unsafe { drop(Box::from_raw(context)) };
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let message = if message.is_empty() {
                "system audio capture requires macOS 14.2 or later".to_owned()
            } else {
                message
            };
            return Err(Error::Audio(message));
        }

        Ok(Self { handle, context })
    }

    pub(crate) fn play(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let mut error = [0_i8; 256];
            // SAFETY: `self.handle` is a live tap returned by `imp::create`.
            let status = unsafe { imp::begin(self.handle, error.as_mut_ptr(), error.len()) };
            if status != 0 {
                let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned();
                return Err(Error::Audio(if message.is_empty() {
                    "could not start the system audio tap".to_owned()
                } else {
                    message
                }));
            }
        }
        Ok(())
    }
}

impl Drop for SystemAudioTap {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: `handle` was returned by the shim and is destroyed exactly once.
        unsafe {
            imp::stop(self.handle);
        }
        if !self.context.is_null() {
            // SAFETY: the shim is stopped, so no callback can use the context.
            unsafe { drop(Box::from_raw(self.context)) };
            self.context = std::ptr::null_mut();
        }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn on_samples(
    samples: *const f32,
    frames: u32,
    channels: u32,
    sample_rate_hz: f64,
    context: *mut std::ffi::c_void,
) {
    if samples.is_null() || context.is_null() || frames == 0 || channels == 0 {
        return;
    }
    let sample_rate_hz = sample_rate_hz as u32;
    if sample_rate_hz == 0 {
        return;
    }
    // SAFETY: the shim passes a valid context pointer and a buffer valid for
    // `frames * channels` samples for the duration of this call.
    let context = unsafe { &*(context as *const TapContext) };
    let sample_count = frames as usize * channels as usize;
    let samples = unsafe { std::slice::from_raw_parts(samples, sample_count) }.to_vec();
    let captured_at_us = context
        .epoch
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);

    if context
        .sender
        .try_send(RawInput {
            source: AudioSourceKind::System,
            captured_at_us,
            sample_rate_hz,
            channels: channels as u16,
            samples,
        })
        .is_err()
    {
        context.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void};

    unsafe extern "C" {
        #[link_name = "scrib_system_audio_available"]
        pub fn available() -> i32;

        #[link_name = "scrib_system_audio_permission"]
        pub fn permission() -> i32;

        #[link_name = "scrib_system_audio_create"]
        pub fn create(
            callback: unsafe extern "C" fn(*const f32, u32, u32, f64, *mut c_void),
            context: *mut c_void,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut c_void;

        #[link_name = "scrib_system_audio_begin"]
        pub fn begin(handle: *mut c_void, error: *mut c_char, error_len: usize) -> i32;

        #[link_name = "scrib_system_audio_stop"]
        pub fn stop(handle: *mut c_void);
    }

    pub const CALLBACK: unsafe extern "C" fn(*const f32, u32, u32, f64, *mut c_void) =
        super::on_samples;
}

#[cfg(all(target_os = "macos", test))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Manual smoke test: plays a system sound and expects the tap to deliver it.
    /// Requires speaker output and the System Audio Recording permission.
    #[test]
    #[ignore = "requires System Audio Recording permission and speaker output"]
    fn tap_captures_system_audio() {
        let (sender, receiver) = crossbeam_channel::bounded(512);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut tap = SystemAudioTap::create(sender, Instant::now(), dropped)
            .expect("the system audio tap should be created");
        tap.play().expect("the system audio tap should start");

        let mut player = std::process::Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .spawn()
            .expect("afplay should start");
        let mut peak = 0.0_f32;
        let mut blocks = 0_u64;
        let mut frames = 0_u64;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(input) = receiver.recv_timeout(Duration::from_millis(100)) {
                blocks += 1;
                frames += input.samples.len() as u64;
                peak = peak.max(
                    input
                        .samples
                        .iter()
                        .fold(0.0_f32, |maximum, sample| maximum.max(sample.abs())),
                );
            }
        }
        let _ = player.wait();
        drop(tap);

        assert!(
            peak > 0.001,
            "the tap delivered {blocks} block(s) / {frames} sample(s); peak {peak}. \
             If blocks are zero, check the tap; if samples are silent, grant System Audio Recording permission"
        );
    }
}
