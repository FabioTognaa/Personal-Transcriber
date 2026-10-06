use std::path::Path;

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

use crate::{Error, Result};

const V1_MODEL_NAME: &str = "ggml-small-q5_1.bin";
const V1_MODEL_SIZE_BYTES: u64 = 190_085_487;
const V1_MODEL_SHA256: &str = "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioDevice {
    pub name: String,
    pub default_input: bool,
    pub default_output: bool,
    pub input: Option<StreamConfiguration>,
    pub output: Option<StreamConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamConfiguration {
    pub channels: u16,
    pub sample_rate_hz: u32,
    pub sample_format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInventory {
    pub host: String,
    pub devices: Vec<AudioDevice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
}

pub fn audio_devices() -> Result<DeviceInventory> {
    let host = cpal::default_host();
    let default_input = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let default_output = host
        .default_output_device()
        .and_then(|device| device.name().ok());
    let mut devices = host
        .devices()
        .map_err(|error| Error::Audio(error.to_string()))?
        .map(|device| {
            let name = device
                .name()
                .map_err(|error| Error::Audio(error.to_string()))?;
            let input = device.default_input_config().ok().map(stream_configuration);
            let output = device
                .default_output_config()
                .ok()
                .map(stream_configuration);
            Ok(AudioDevice {
                default_input: default_input.as_deref() == Some(name.as_str()),
                default_output: default_output.as_deref() == Some(name.as_str()),
                name,
                input,
                output,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    devices.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(DeviceInventory {
        host: format!("{:?}", host.id()),
        devices,
    })
}

pub fn doctor(sessions_dir: &Path, model: &Path) -> DoctorReport {
    let mut checks = vec![platform_check(), sessions_directory_check(sessions_dir)];
    checks.push(disk_space_check(sessions_dir));
    checks.push(model_check(model));
    checks.extend(device_checks());
    checks.push(DoctorCheck {
        name: "microphone_permission",
        status: CheckStatus::Warning,
        detail: "not probed: doctor does not open an audio stream; capture will verify it in M5"
            .to_owned(),
    });
    let ready = checks.iter().all(|check| check.status != CheckStatus::Fail);

    DoctorReport { ready, checks }
}

fn stream_configuration(config: cpal::SupportedStreamConfig) -> StreamConfiguration {
    StreamConfiguration {
        channels: config.channels(),
        sample_rate_hz: config.sample_rate().0,
        sample_format: format!("{:?}", config.sample_format()).to_lowercase(),
    }
}

fn platform_check() -> DoctorCheck {
    let architecture = std::env::consts::ARCH;
    let operating_system = std::env::consts::OS;
    let supported = operating_system == "macos" && architecture == "aarch64";
    DoctorCheck {
        name: "platform",
        status: if supported {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: format!(
            "{operating_system}/{architecture}; v1 requires macos/aarch64 (Apple Silicon)"
        ),
    }
}

fn sessions_directory_check(sessions_dir: &Path) -> DoctorCheck {
    match crate::validation::validate_directory_target(sessions_dir, "sessions directory") {
        Ok(existing) => DoctorCheck {
            name: "sessions_directory",
            status: CheckStatus::Pass,
            detail: format!(
                "{} uses existing filesystem at {}",
                sessions_dir.display(),
                existing.display()
            ),
        },
        Err(error) => DoctorCheck {
            name: "sessions_directory",
            status: CheckStatus::Fail,
            detail: error.to_string(),
        },
    }
}

fn disk_space_check(sessions_dir: &Path) -> DoctorCheck {
    match crate::validation::available_disk_space(sessions_dir) {
        Ok(available) => {
            let status = if available >= crate::validation::DOCTOR_RECOMMENDED_FREE_BYTES {
                CheckStatus::Pass
            } else if available >= crate::validation::SESSION_DISK_HEADROOM_BYTES {
                CheckStatus::Warning
            } else {
                CheckStatus::Fail
            };
            DoctorCheck {
                name: "disk_space",
                status,
                detail: format!(
                    "{available} bytes available; {} bytes recommended",
                    crate::validation::DOCTOR_RECOMMENDED_FREE_BYTES
                ),
            }
        }
        Err(error) => DoctorCheck {
            name: "disk_space",
            status: CheckStatus::Fail,
            detail: error.to_string(),
        },
    }
}

fn model_check(model: &Path) -> DoctorCheck {
    match crate::validation::validate_existing_file(model, "model")
        .and_then(|_| crate::asr::model_identity(model))
    {
        Ok(identity) => {
            let expected_v1_model = identity.name == V1_MODEL_NAME;
            let matches_v1 =
                identity.size_bytes == V1_MODEL_SIZE_BYTES && identity.sha256 == V1_MODEL_SHA256;
            DoctorCheck {
                name: "model",
                status: if !expected_v1_model || matches_v1 {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: if expected_v1_model && !matches_v1 {
                    format!(
                        "{} does not match the documented v1 model: got {} bytes, sha256 {}",
                        identity.path.display(),
                        identity.size_bytes,
                        identity.sha256
                    )
                } else {
                    format!(
                        "{} ({} bytes, sha256 {})",
                        identity.path.display(),
                        identity.size_bytes,
                        identity.sha256
                    )
                },
            }
        }
        Err(error) => DoctorCheck {
            name: "model",
            status: CheckStatus::Fail,
            detail: error.to_string(),
        },
    }
}

fn device_checks() -> Vec<DoctorCheck> {
    match audio_devices() {
        Ok(inventory) => {
            let input_count = inventory
                .devices
                .iter()
                .filter(|device| device.input.is_some())
                .count();
            let blackhole = inventory
                .devices
                .iter()
                .find(|device| device.input.is_some() && is_blackhole(&device.name));
            vec![
                DoctorCheck {
                    name: "audio_inputs",
                    status: if input_count > 0 {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    detail: format!("{input_count} input device(s) visible through cpal"),
                },
                DoctorCheck {
                    name: "blackhole",
                    status: if blackhole.is_some() {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    detail: blackhole.map_or_else(
                        || "no BlackHole input device is visible".to_owned(),
                        |device| format!("found input device `{}`", device.name),
                    ),
                },
            ]
        }
        Err(error) => vec![
            DoctorCheck {
                name: "audio_inputs",
                status: CheckStatus::Fail,
                detail: error.to_string(),
            },
            DoctorCheck {
                name: "blackhole",
                status: CheckStatus::Fail,
                detail: "device enumeration failed".to_owned(),
            },
        ],
    }
}

fn is_blackhole(name: &str) -> bool {
    name.to_ascii_lowercase().contains("blackhole")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blackhole_matching_is_case_insensitive() {
        assert!(is_blackhole("BlackHole 2ch"));
        assert!(is_blackhole("blackhole 16ch"));
        assert!(!is_blackhole("MacBook Pro Microphone"));
    }

    #[test]
    fn missing_model_is_a_failed_check() {
        let check = model_check(Path::new("/definitely/missing/model.bin"));

        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn corrupted_documented_model_is_a_failed_check() {
        let temporary = tempfile::tempdir().expect("temp directory should be created");
        let model = temporary.path().join(V1_MODEL_NAME);
        std::fs::write(&model, b"not the documented model").expect("fixture should be written");

        assert_eq!(model_check(&model).status, CheckStatus::Fail);
    }
}
