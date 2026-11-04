//! Model presets, on-disk artifacts and the table of documented models.
//!
//! A model is either a single GGML file (Whisper) or a directory bundle
//! (Parakeet). The user selects a model; its artifact kind decides the engine.
//! The known-model table lets `doctor` verify a downloaded artifact instead of
//! trusting any file that happens to share the name.

use std::path::{Path, PathBuf};

use clap::ValueEnum;

use crate::domain::EngineKind;
use crate::{Error, Result};

/// Directory, relative to the working directory, where model artifacts live.
pub const MODELS_DIR: &str = "models";
/// Environment variable that overrides [`MODELS_DIR`].
pub const MODELS_DIR_ENV: &str = "SCRIB_MODELS_DIR";

/// Directory that holds model artifacts.
#[must_use]
pub fn models_dir() -> PathBuf {
    std::env::var_os(MODELS_DIR_ENV).map_or_else(|| PathBuf::from(MODELS_DIR), PathBuf::from)
}

/// Built-in model choices exposed as `--model-preset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModelPreset {
    /// Whisper small, Q5_1 (~190 MB). The original v1 model.
    Small,
    /// Whisper medium, Q5_0 (~540 MB).
    Medium,
    /// Whisper large-v3-turbo, Q8_0 (~874 MB). Best Whisper quality/speed trade-off.
    LargeV3Turbo,
    /// Whisper large-v3, Q5_0 (~1.1 GB). Highest Whisper quality, heaviest option.
    LargeV3,
    /// NVIDIA Parakeet TDT 0.6B v3, int8 ONNX bundle (~670 MB), 25 European languages.
    ParakeetV3,
}

impl ModelPreset {
    /// All presets, in declaration order.
    pub const ALL: [ModelPreset; 5] = [
        Self::Small,
        Self::Medium,
        Self::LargeV3Turbo,
        Self::LargeV3,
        Self::ParakeetV3,
    ];

    /// File or directory name of the documented artifact for this preset.
    #[must_use]
    pub const fn artifact_name(self) -> &'static str {
        match self {
            Self::Small => "ggml-small-q5_1.bin",
            Self::Medium => "ggml-medium-q5_0.bin",
            Self::LargeV3Turbo => "ggml-large-v3-turbo-q8_0.bin",
            Self::LargeV3 => "ggml-large-v3-q5_0.bin",
            Self::ParakeetV3 => "parakeet-tdt-0.6b-v3-onnx",
        }
    }

    /// CLI-facing name, matching the `ValueEnum` kebab-case value.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::LargeV3Turbo => "large-v3-turbo",
            Self::LargeV3 => "large-v3",
            Self::ParakeetV3 => "parakeet-v3",
        }
    }

    /// Path where the preset is looked up by default.
    #[must_use]
    pub fn default_path(self) -> PathBuf {
        models_dir().join(self.artifact_name())
    }
}

/// How a model is stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// One self-contained file, as used by whisper.cpp.
    SingleFile,
    /// A directory with several files, as used by Parakeet.
    Bundle,
}

impl ArtifactKind {
    /// Engine that runs this kind of artifact.
    #[must_use]
    pub const fn engine(self) -> EngineKind {
        match self {
            Self::SingleFile => EngineKind::Whisper,
            Self::Bundle => EngineKind::Parakeet,
        }
    }
}

/// A model ready to be handed to an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelArtifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
}

impl ModelArtifact {
    /// Detect whether `path` is a single-file or a Parakeet bundle artifact.
    ///
    /// Detection is structural: a regular non-empty file is a Whisper artifact;
    /// a directory with the Parakeet files is a bundle. Anything else is an error.
    pub fn detect(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| Error::InvalidPath {
            label: "model",
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        if metadata.is_file() {
            if metadata.len() == 0 {
                return Err(Error::InvalidPath {
                    label: "model",
                    path: path.to_path_buf(),
                    reason: "file is empty".to_owned(),
                });
            }
            return Ok(Self {
                kind: ArtifactKind::SingleFile,
                path: path.to_path_buf(),
            });
        }
        if !metadata.is_dir() {
            return Err(Error::InvalidPath {
                label: "model",
                path: path.to_path_buf(),
                reason: "expected a model file or a model directory".to_owned(),
            });
        }
        for required in [
            PARAKEET_CONFIG,
            PARAKEET_VOCAB,
            PARAKEET_ENCODER,
            PARAKEET_DECODER_JOINT,
        ] {
            if !path.join(required).is_file() {
                return Err(Error::InvalidPath {
                    label: "model",
                    path: path.to_path_buf(),
                    reason: format!("missing {required}"),
                });
            }
        }
        if preprocessor_file(path).is_none() {
            return Err(Error::InvalidPath {
                label: "model",
                path: path.to_path_buf(),
                reason: "missing nemo*.onnx preprocessor".to_owned(),
            });
        }
        Ok(Self {
            kind: ArtifactKind::Bundle,
            path: path.to_path_buf(),
        })
    }

    /// Name used in transcripts and in `doctor` output.
    #[must_use]
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Files that make up the artifact, in a deterministic order.
    pub fn files(&self) -> Result<Vec<PathBuf>> {
        match self.kind {
            ArtifactKind::SingleFile => Ok(vec![self.path.clone()]),
            ArtifactKind::Bundle => {
                let preprocessor =
                    preprocessor_file(&self.path).ok_or_else(|| Error::InvalidPath {
                        label: "model",
                        path: self.path.clone(),
                        reason: "missing nemo*.onnx preprocessor".to_owned(),
                    })?;
                let mut files = vec![
                    self.path.join(PARAKEET_CONFIG),
                    self.path.join(PARAKEET_VOCAB),
                    preprocessor,
                    self.path.join(PARAKEET_ENCODER),
                    self.path.join(PARAKEET_DECODER_JOINT),
                ];
                files.sort();
                Ok(files)
            }
        }
    }

    /// Total size of the files that make up the artifact.
    pub fn total_size(&self) -> Result<u64> {
        self.files()?.into_iter().try_fold(0_u64, |total, path| {
            let size = std::fs::metadata(&path)
                .map_err(|error| Error::InvalidPath {
                    label: "model",
                    path: path.clone(),
                    reason: error.to_string(),
                })?
                .len();
            Ok(total.saturating_add(size))
        })
    }
}

pub const PARAKEET_CONFIG: &str = "config.json";
pub const PARAKEET_VOCAB: &str = "vocab.txt";
pub const PARAKEET_ENCODER: &str = "encoder-model.int8.onnx";
pub const PARAKEET_DECODER_JOINT: &str = "decoder_joint-model.int8.onnx";

/// The mel preprocessor shipped with a Parakeet bundle, e.g. `nemo128.onnx`.
fn preprocessor_file(directory: &Path) -> Option<PathBuf> {
    let mut candidates = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("nemo") && name.ends_with(".onnx"))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

/// One file of a documented model artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFile {
    pub name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
}

/// A documented model artifact, verified by `doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownModel {
    pub name: &'static str,
    pub kind: ArtifactKind,
    pub files: &'static [KnownFile],
}

/// Models with a documented size and SHA-256, verified by `doctor`.
///
/// Values for Whisper come from the Hugging Face `ggerganov/whisper.cpp`
/// repository: the LFS object id of each file is its SHA-256. The Parakeet
/// bundle comes from `istupakov/parakeet-tdt-0.6b-v3-onnx` (CC-BY-4.0).
pub const KNOWN_MODELS: &[KnownModel] = &[
    KnownModel {
        name: "ggml-small-q5_1.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-small-q5_1.bin",
            size_bytes: 190_085_487,
            sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        }],
    },
    KnownModel {
        name: "ggml-small-q8_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-small-q8_0.bin",
            size_bytes: 264_464_607,
            sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
        }],
    },
    KnownModel {
        name: "ggml-small.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-small.bin",
            size_bytes: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }],
    },
    KnownModel {
        name: "ggml-medium-q5_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-medium-q5_0.bin",
            size_bytes: 539_212_467,
            sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        }],
    },
    KnownModel {
        name: "ggml-medium-q8_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-medium-q8_0.bin",
            size_bytes: 823_369_779,
            sha256: "42a1ffcbe4167d224232443396968db4d02d4e8e87e213d3ee2e03095dea6502",
        }],
    },
    KnownModel {
        name: "ggml-medium.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-medium.bin",
            size_bytes: 1_533_763_059,
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        }],
    },
    KnownModel {
        name: "ggml-large-v3-turbo-q5_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-large-v3-turbo-q5_0.bin",
            size_bytes: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        }],
    },
    KnownModel {
        name: "ggml-large-v3-turbo-q8_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-large-v3-turbo-q8_0.bin",
            size_bytes: 874_188_075,
            sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        }],
    },
    KnownModel {
        name: "ggml-large-v3-turbo.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-large-v3-turbo.bin",
            size_bytes: 1_624_555_275,
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        }],
    },
    KnownModel {
        name: "ggml-large-v3-q5_0.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-large-v3-q5_0.bin",
            size_bytes: 1_081_140_203,
            sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
        }],
    },
    KnownModel {
        name: "ggml-large-v3.bin",
        kind: ArtifactKind::SingleFile,
        files: &[KnownFile {
            name: "ggml-large-v3.bin",
            size_bytes: 3_095_033_483,
            sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        }],
    },
    KnownModel {
        name: "parakeet-tdt-0.6b-v3-onnx",
        kind: ArtifactKind::Bundle,
        files: &[
            KnownFile {
                name: "config.json",
                size_bytes: 97,
                sha256: "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
            },
            KnownFile {
                name: "nemo128.onnx",
                size_bytes: 139_764,
                sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
            },
            KnownFile {
                name: "encoder-model.int8.onnx",
                size_bytes: 652_183_999,
                sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
            },
            KnownFile {
                name: "decoder_joint-model.int8.onnx",
                size_bytes: 18_202_004,
                sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
            },
            KnownFile {
                name: "vocab.txt",
                size_bytes: 93_939,
                sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            },
        ],
    },
];

/// Look up the documented artifact for a model name, if it is known.
#[must_use]
pub fn known_model(name: &str) -> Option<&'static KnownModel> {
    KNOWN_MODELS.iter().find(|model| model.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_resolve_under_the_models_directory() {
        assert_eq!(
            ModelPreset::Small.default_path(),
            models_dir().join("ggml-small-q5_1.bin")
        );
        assert_eq!(
            ModelPreset::LargeV3Turbo.default_path(),
            models_dir().join("ggml-large-v3-turbo-q8_0.bin")
        );
        assert_eq!(
            ModelPreset::ParakeetV3.default_path(),
            models_dir().join("parakeet-tdt-0.6b-v3-onnx")
        );
    }

    #[test]
    fn documented_v1_model_is_known() {
        let model = known_model("ggml-small-q5_1.bin").expect("the v1 model is documented");

        assert_eq!(model.kind, ArtifactKind::SingleFile);
        assert_eq!(model.files[0].size_bytes, 190_085_487);
        assert_eq!(
            model.files[0].sha256,
            "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"
        );
    }

    #[test]
    fn unknown_model_is_not_in_the_table() {
        assert!(known_model("custom-model.bin").is_none());
    }

    #[test]
    fn a_directory_without_the_bundle_files_is_not_an_artifact() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        std::fs::write(temporary.path().join("random.txt"), b"x").expect("fixture");

        assert!(ModelArtifact::detect(temporary.path()).is_err());
    }

    #[test]
    fn a_complete_bundle_is_detected_and_lists_its_files() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        for name in [
            PARAKEET_CONFIG,
            PARAKEET_VOCAB,
            "nemo128.onnx",
            PARAKEET_ENCODER,
            PARAKEET_DECODER_JOINT,
        ] {
            std::fs::write(temporary.path().join(name), b"x").expect("fixture");
        }

        let artifact = ModelArtifact::detect(temporary.path()).expect("bundle should be detected");

        assert_eq!(artifact.kind, ArtifactKind::Bundle);
        assert_eq!(artifact.files().expect("files").len(), 5);
        assert_eq!(artifact.total_size().expect("size"), 5);
    }

    #[test]
    fn a_regular_file_is_a_single_file_artifact() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let model = temporary.path().join("model.bin");
        std::fs::write(&model, b"model").expect("fixture");

        let artifact = ModelArtifact::detect(&model).expect("file should be detected");

        assert_eq!(artifact.kind, ArtifactKind::SingleFile);
        assert_eq!(artifact.kind.engine(), EngineKind::Whisper);
    }
}
