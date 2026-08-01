//! Immutable catalog for downloadable local dictation models.
//!
//! Mutable installation, selection, and runtime state deliberately live in
//! other modules. A future model should be introduced here plus in the runtime
//! adapter that can execute its `runtime` kind.

use serde::Serialize;

pub const PARAKEET_Q8_PREF: &str = "parakeet-en-q8";
pub const NEMOTRON_Q4_PREF: &str = "nemotron-q4";
pub const NEMOTRON_Q8_PREF: &str = "nemotron-q8";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    TranscribeCpp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub translate: bool,
    pub language_detection: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LocalModelDescriptor {
    pub key: &'static str,
    pub name: &'static str,
    pub family: &'static str,
    pub architecture: &'static str,
    pub runtime: RuntimeKind,
    pub tier: ModelTier,
    pub languages: &'static [&'static str],
    pub capabilities: ModelCapabilities,
    pub repository: &'static str,
    pub revision: &'static str,
    pub filename: &'static str,
    pub quantization: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub license: &'static str,
    pub attribution: &'static str,
    pub minimum_memory_bytes: u64,
}

impl LocalModelDescriptor {
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repository, self.revision, self.filename
        )
    }

    pub fn mirror_url(&self) -> String {
        format!(
            "https://blob.handy.computer/{}/{}/{}",
            self.repository, self.revision, self.filename
        )
    }

    pub fn size_hint(&self) -> String {
        format!("~{} MB", self.size_bytes.div_ceil(1_000_000))
    }

    pub fn supports_language(&self, requested: &str) -> bool {
        let requested = requested.trim().to_ascii_lowercase();
        match requested.as_str() {
            "en" | "english" => self.languages.contains(&"en"),
            "hi" | "hindi" => self.languages.contains(&"hi"),
            "hinglish" => self.languages.contains(&"en") && self.languages.contains(&"hi"),
            "" | "auto" => self.capabilities.language_detection,
            value => self.languages.contains(&value),
        }
    }
}

const HALF_GIB: u64 = 512 * 1024 * 1024;
const ONE_GIB: u64 = 1024 * 1024 * 1024;

pub static MODELS: &[LocalModelDescriptor] = &[
    LocalModelDescriptor {
        key: PARAKEET_Q8_PREF,
        name: "Parakeet Unified EN 0.6B (Q8)",
        family: "parakeet",
        architecture: "parakeet",
        runtime: RuntimeKind::TranscribeCpp,
        tier: ModelTier::Medium,
        languages: &["en"],
        capabilities: ModelCapabilities {
            // The base model is unified streaming/offline, but this pinned
            // GGUF conversion currently exposes offline inference only.
            streaming: false,
            translate: false,
            language_detection: false,
        },
        repository: "handy-computer/parakeet-unified-en-0.6b-gguf",
        revision: "7e948f21b7bdbac698d3318db9d350f1096f3b6c",
        filename: "parakeet-unified-en-0.6b-Q8_0.gguf",
        quantization: "Q8_0",
        size_bytes: 731_357_568,
        sha256: "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795",
        license: "cc-by-4.0",
        attribution: "NVIDIA Parakeet Unified EN 0.6B; GGUF conversion by handy-computer",
        minimum_memory_bytes: ONE_GIB,
    },
    LocalModelDescriptor {
        key: NEMOTRON_Q4_PREF,
        name: "Nemotron Streaming 3.5 (Q4)",
        family: "nemotron",
        architecture: "parakeet",
        runtime: RuntimeKind::TranscribeCpp,
        tier: ModelTier::Medium,
        languages: &[
            "en", "es", "fr", "it", "pt", "nl", "de", "tr", "ru", "ar", "hi", "ja", "ko", "vi",
            "uk", "pl", "sv", "cs", "nb", "da", "bg", "fi", "hr", "sk", "zh", "hu", "ro", "et",
        ],
        capabilities: ModelCapabilities {
            streaming: true,
            translate: false,
            language_detection: true,
        },
        repository: "handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf",
        revision: "6d44e540bc31b0de1dbe174a3cea87f53a7f22fb",
        filename: "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
        quantization: "Q4_K_M",
        size_bytes: 495_831_520,
        sha256: "41c99fa5fb6f3d35f68e79adc3e755eca2232a8d921178bd647b71194792b8fd",
        license: "nvidia-open-model-license",
        attribution: "NVIDIA Nemotron 3.5 ASR Streaming 0.6B; GGUF conversion by handy-computer",
        minimum_memory_bytes: HALF_GIB,
    },
    LocalModelDescriptor {
        key: NEMOTRON_Q8_PREF,
        name: "Nemotron Streaming 3.5 (Q8)",
        family: "nemotron",
        architecture: "parakeet",
        runtime: RuntimeKind::TranscribeCpp,
        tier: ModelTier::Medium,
        languages: &[
            "en", "es", "fr", "it", "pt", "nl", "de", "tr", "ru", "ar", "hi", "ja", "ko", "vi",
            "uk", "pl", "sv", "cs", "nb", "da", "bg", "fi", "hr", "sk", "zh", "hu", "ro", "et",
        ],
        capabilities: ModelCapabilities {
            streaming: true,
            translate: false,
            language_detection: true,
        },
        repository: "handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf",
        revision: "6d44e540bc31b0de1dbe174a3cea87f53a7f22fb",
        filename: "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf",
        quantization: "Q8_0",
        size_bytes: 751_094_240,
        sha256: "b94545b313b3223fda7b2857a52681da813935c2127643d1e9ff0c23d988089c",
        license: "nvidia-open-model-license",
        attribution: "NVIDIA Nemotron 3.5 ASR Streaming 0.6B; GGUF conversion by handy-computer",
        minimum_memory_bytes: ONE_GIB,
    },
];

pub fn canonical_key(value: &str) -> &str {
    match value {
        "nemotron" => NEMOTRON_Q8_PREF,
        other => other,
    }
}

pub fn find(value: &str) -> Option<&'static LocalModelDescriptor> {
    let key = canonical_key(value);
    MODELS.iter().find(|model| model.key == key)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_identity_and_integrity_metadata_are_valid() {
        let mut keys = HashSet::new();
        for model in MODELS {
            assert!(keys.insert(model.key), "duplicate model key: {}", model.key);
            assert_eq!(model.revision.len(), 40, "revision: {}", model.key);
            assert!(model.revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert_eq!(model.sha256.len(), 64, "sha256: {}", model.key);
            assert!(model.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(model.size_bytes > 0);
            assert!(model.download_url().contains(model.revision));
        }
    }

    #[test]
    fn english_only_model_rejects_auto_hindi_and_hinglish() {
        let parakeet = find(PARAKEET_Q8_PREF).unwrap();
        assert!(parakeet.supports_language("english"));
        assert!(!parakeet.supports_language("auto"));
        assert!(!parakeet.supports_language("hindi"));
        assert!(!parakeet.supports_language("hinglish"));
    }

    #[test]
    fn legacy_nemotron_key_stays_q8_compatible() {
        assert_eq!(find("nemotron").unwrap().key, NEMOTRON_Q8_PREF);
    }
}
