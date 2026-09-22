//! What the local dictation model is, and whether the copy on this machine is
//! still the right one.
//!
//! The model file itself lives where it always has — `whisper_model_path()` —
//! so the STT runtime, the backend loader, and every caller stay untouched.
//! What changed is that AirNote no longer hardcodes a public Hugging Face URL.
//! The current model lives in a private repository, so the desktop asks the
//! control plane for a short-lived signed URL instead and the token stays on
//! the server.
//!
//! Because the path is unchanged, an older Oriserve file and the current model
//! occupy the same filename, and their sizes are close enough (~148 MB vs
//! ~141 MB) that size cannot tell them apart. So every successful install drops
//! a small sidecar next to the model recording which revision it is. No
//! sidecar, or a sidecar naming a different revision, means the file predates
//! this release and must be replaced.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the control plane is, mirroring the diagnostics/base-URL convention
/// used in `main.rs` so a self-hosted deployment can point all of it at once.
const DEFAULT_CONTROL_PLANE: &str = "https://airnote.emiactech.com";

pub fn control_plane_base() -> String {
    std::env::var("AIRNOTE_CONTROL_PLANE_URL")
        .or_else(|_| std::env::var("AIRNOTE_DIAGNOSTICS_URL"))
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// What the server says the current dictation model is.
///
/// `url` is signed and short-lived — fetch it immediately before downloading,
/// never cache it across launches.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DictationModelDescriptor {
    pub key: String,
    pub name: String,
    pub filename: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub revision: String,
}

/// Written beside the model after a verified install. Small on purpose: this
/// only has to answer "is the file on disk the revision we expect".
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledModelMarker {
    pub key: String,
    pub revision: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Ask the control plane which model to install and where to get it.
pub async fn fetch_descriptor() -> Result<DictationModelDescriptor, String> {
    let url = format!("{}/v1/models/dictation", control_plane_base());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("couldn't build HTTP client: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("couldn't reach AirNote's model service: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        // The server deliberately keeps its message vague; surface it as-is
        // rather than inventing a friendlier one that hides the cause.
        let body = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(format!("Model service refused the request: {detail}"));
    }

    response
        .json::<DictationModelDescriptor>()
        .await
        .map_err(|e| format!("model service sent an unreadable response: {e}"))
}

/// Path of the sidecar that records which revision is installed.
pub fn marker_path() -> PathBuf {
    marker_path_for(&said_core::paths::whisper_model_path())
}

fn marker_path_for(model: &Path) -> PathBuf {
    model.with_extension("bin.installed.json")
}

pub fn read_marker() -> Option<InstalledModelMarker> {
    let raw = std::fs::read_to_string(marker_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_marker(descriptor: &DictationModelDescriptor) -> Result<(), String> {
    let marker = InstalledModelMarker {
        key: descriptor.key.clone(),
        revision: descriptor.revision.clone(),
        sha256: descriptor.sha256.clone(),
        size_bytes: descriptor.size_bytes,
    };
    let raw = serde_json::to_string_pretty(&marker)
        .map_err(|e| format!("couldn't serialise the model marker: {e}"))?;
    std::fs::write(marker_path(), raw)
        .map_err(|e| format!("couldn't record which model is installed: {e}"))
}

pub fn clear_marker() {
    let _ = std::fs::remove_file(marker_path());
}

/// Why the on-disk model does or does not need replacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    /// No model file at all — a first install.
    Missing,
    /// A model file exists but carries no marker, so it predates this release.
    /// Every existing user upgrading from 2.4.5 lands here.
    Legacy,
    /// A marker exists but names a different revision than the server wants.
    Outdated,
    /// On disk, marked, and matching the server.
    Current,
}

impl ModelState {
    pub fn needs_download(self) -> bool {
        !matches!(self, ModelState::Current)
    }

    /// Whether the existing file has to be removed before downloading. A
    /// partial or foreign file at the destination would otherwise be resumed
    /// into, producing a corrupt model that still looks the right size.
    pub fn needs_cleanup(self) -> bool {
        matches!(self, ModelState::Legacy | ModelState::Outdated)
    }
}

/// Compare what is on disk against what the server says should be there.
pub fn state_for(descriptor: &DictationModelDescriptor) -> ModelState {
    let path = said_core::paths::whisper_model_path();
    state_from(path.is_file(), read_marker().as_ref(), &descriptor.revision)
}

/// Split out so the decision is testable without touching the filesystem.
fn state_from(
    file_exists: bool,
    marker: Option<&InstalledModelMarker>,
    wanted_revision: &str,
) -> ModelState {
    if !file_exists {
        return ModelState::Missing;
    }
    match marker {
        None => ModelState::Legacy,
        Some(marker) if marker.revision == wanted_revision => ModelState::Current,
        Some(_) => ModelState::Outdated,
    }
}

/// SHA-256 of a file, so a download can be proven rather than assumed.
pub fn sha256_of(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("couldn't open the model file: {e}"))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| format!("couldn't read the model file: {e}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(revision: &str) -> InstalledModelMarker {
        InstalledModelMarker {
            key: "clario-hinglish-41h".into(),
            revision: revision.into(),
            sha256: "abc".into(),
            size_bytes: 1,
        }
    }

    #[test]
    fn no_file_means_first_install() {
        assert_eq!(state_from(false, None, "rev1"), ModelState::Missing);
    }

    #[test]
    fn a_file_without_a_marker_is_the_pre_release_model() {
        // This is the 2.4.5 upgrade path: Oriserve sits at the same filename.
        assert_eq!(state_from(true, None, "rev1"), ModelState::Legacy);
        assert!(ModelState::Legacy.needs_download());
        assert!(ModelState::Legacy.needs_cleanup());
    }

    #[test]
    fn a_marker_naming_another_revision_is_outdated() {
        assert_eq!(
            state_from(true, Some(&marker("old")), "rev1"),
            ModelState::Outdated
        );
        assert!(ModelState::Outdated.needs_cleanup());
    }

    #[test]
    fn a_matching_marker_needs_no_work() {
        let state = state_from(true, Some(&marker("rev1")), "rev1");
        assert_eq!(state, ModelState::Current);
        assert!(!state.needs_download());
        assert!(!state.needs_cleanup());
    }

    #[test]
    fn missing_needs_a_download_but_nothing_to_clean() {
        assert!(ModelState::Missing.needs_download());
        assert!(!ModelState::Missing.needs_cleanup());
    }

    #[test]
    fn the_marker_sits_beside_the_model() {
        let marker = marker_path_for(Path::new("/models/ggml-model.bin"));
        assert_eq!(
            marker,
            PathBuf::from("/models/ggml-model.bin.installed.json")
        );
    }

    #[test]
    fn control_plane_base_has_no_trailing_slash() {
        // Callers append "/v1/..."; a trailing slash would produce a double.
        assert!(!control_plane_base().ends_with('/'));
    }
}
