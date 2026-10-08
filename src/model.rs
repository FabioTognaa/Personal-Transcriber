//! Whisper model presets and the table of documented model artifacts.
//!
//! A preset turns a stable CLI name (`--model-preset`) into a path under
//! `models/`. The known-model table lets `doctor` verify that a downloaded file
//! is the documented artifact (name, size and SHA-256) instead of trusting any
//! non-empty file that happens to share the name.

use std::path::{Path, PathBuf};

use clap::ValueEnum;

/// Directory, relative to the working directory, where GGML models are stored.
pub const MODELS_DIR: &str = "models";

/// Built-in model choices exposed as `--model-preset`.
///
/// Presets are a convenience over `--model <path>`: they resolve to a documented
/// file under [`MODELS_DIR`] and describe the default quantization. Any other
/// whisper.cpp GGML file can still be selected explicitly with `--model`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModelPreset {
    /// Whisper small, Q5_1 (~190 MB). The original v1 model.
    Small,
    /// Whisper medium, Q5_0 (~540 MB).
    Medium,
    /// Whisper large-v3-turbo, Q8_0 (~874 MB). Best quality/speed trade-off.
    LargeV3Turbo,
    /// Whisper large-v3, Q5_0 (~1.1 GB). Highest quality, heaviest option.
    LargeV3,
}

impl ModelPreset {
    /// All presets, in the order shown by the interactive menu.
    pub const ALL: [ModelPreset; 4] =
        [Self::Small, Self::Medium, Self::LargeV3Turbo, Self::LargeV3];

    /// File name of the documented artifact for this preset.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Small => "ggml-small-q5_1.bin",
            Self::Medium => "ggml-medium-q5_0.bin",
            Self::LargeV3Turbo => "ggml-large-v3-turbo-q8_0.bin",
            Self::LargeV3 => "ggml-large-v3-q5_0.bin",
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
        }
    }

    /// Path where the preset is looked up by default.
    #[must_use]
    pub fn default_path(self) -> PathBuf {
        Path::new(MODELS_DIR).join(self.file_name())
    }
}

/// A model file published by the `ggerganov/whisper.cpp` GGML repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownModel {
    pub file_name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
}

/// Models with a documented size and SHA-256, verified by `doctor`.
///
/// Values come from the Hugging Face `ggerganov/whisper.cpp` repository: the LFS
/// object id of each file is its SHA-256.
pub const KNOWN_MODELS: &[KnownModel] = &[
    // Whisper small.
    KnownModel {
        file_name: "ggml-small-q5_1.bin",
        size_bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
    KnownModel {
        file_name: "ggml-small-q8_0.bin",
        size_bytes: 264_464_607,
        sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
    },
    KnownModel {
        file_name: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    // Whisper medium.
    KnownModel {
        file_name: "ggml-medium-q5_0.bin",
        size_bytes: 539_212_467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
    },
    KnownModel {
        file_name: "ggml-medium-q8_0.bin",
        size_bytes: 823_369_779,
        sha256: "42a1ffcbe4167d224232443396968db4d02d4e8e87e213d3ee2e03095dea6502",
    },
    KnownModel {
        file_name: "ggml-medium.bin",
        size_bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    // Whisper large-v3-turbo.
    KnownModel {
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    KnownModel {
        file_name: "ggml-large-v3-turbo-q8_0.bin",
        size_bytes: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
    },
    KnownModel {
        file_name: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    // Whisper large-v3.
    KnownModel {
        file_name: "ggml-large-v3-q5_0.bin",
        size_bytes: 1_081_140_203,
        sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    },
    KnownModel {
        file_name: "ggml-large-v3.bin",
        size_bytes: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
];

/// Look up the documented artifact for a model file name, if it is known.
#[must_use]
pub fn known_model(file_name: &str) -> Option<&'static KnownModel> {
    KNOWN_MODELS
        .iter()
        .find(|model| model.file_name == file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_resolve_under_the_models_directory() {
        assert_eq!(
            ModelPreset::Small.default_path(),
            PathBuf::from("models/ggml-small-q5_1.bin")
        );
        assert_eq!(
            ModelPreset::LargeV3Turbo.default_path(),
            PathBuf::from("models/ggml-large-v3-turbo-q8_0.bin")
        );
    }

    #[test]
    fn documented_v1_model_is_known() {
        let model = known_model("ggml-small-q5_1.bin").expect("the v1 model is documented");

        assert_eq!(model.size_bytes, 190_085_487);
        assert_eq!(
            model.sha256,
            "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"
        );
    }

    #[test]
    fn unknown_model_is_not_in_the_table() {
        assert!(known_model("custom-model.bin").is_none());
    }
}
