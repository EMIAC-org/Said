//! Device policy for dictation speech recognition.
//!
//! This is intentionally the only place that decides which speech setup a
//! machine receives. Onboarding, the post-update gate, Settings, and the hot
//! dictation path all consume this policy; none of them infer hardware on their
//! own.

use std::sync::OnceLock;

use serde::Serialize;
use sysinfo::System;

use said_core::prefs::DesktopPrefs;

pub const CLOUD_DEEPINFRA_PREF: &str = "cloud-deepinfra-whisper-v3-turbo";
const LEGACY_CLOUD_NEMOTRON_PREF: &str = "cloud-nemotron-3.5";
pub const LOCAL_PREF: &str = "local";

/// The one local dictation model AirNote ships. Every Apple Silicon Mac gets
/// this regardless of memory: it is a whisper-base fine-tune at ~141 MB, so the
/// memory tiering the previous models needed no longer applies.
pub const CLARIO_41H_PREF: &str = "clario-hinglish-41h";

/// Retired models. Kept as constants only so `local_models` can recognise and
/// reclaim what older installs left on disk; nothing selects them any more.
pub const ORISERVE_PREF: &str = "oriserve";
pub const NEMOTRON_Q4_PREF: &str = "nemotron-q4";

/// What AirNote can expose for dictation on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupKind {
    /// Windows and Intel Macs always use hosted DeepInfra Whisper.
    CloudLocked,
    /// Apple Silicon Macs receive a required local model during setup.
    LocalRequired,
}

/// Immutable hardware-derived policy, safe to send directly to the desktop UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SttSetupPolicy {
    pub platform: String,
    pub cpu_family: String,
    pub total_memory_bytes: u64,
    pub setup_kind: SetupKind,
    /// `None` for cloud-locked devices.
    pub local_model: Option<String>,
    pub local_model_name: Option<String>,
    pub local_model_size_hint: Option<String>,
}

impl SttSetupPolicy {
    pub fn is_cloud_locked(&self) -> bool {
        self.setup_kind == SetupKind::CloudLocked
    }

    pub fn allows_cloud_toggle(&self) -> bool {
        self.setup_kind == SetupKind::LocalRequired
    }

    pub fn local_pref(&self) -> Option<&str> {
        self.local_model.as_deref()
    }
}

/// Return the immutable policy for this launch. Hardware does not change while
/// AirNote is running, so cache it instead of probing on every dictation.
pub fn current() -> &'static SttSetupPolicy {
    static POLICY: OnceLock<SttSetupPolicy> = OnceLock::new();
    POLICY.get_or_init(detect)
}

/// Normalize a preference record to the policy without changing an explicit
/// Apple-Silicon cloud choice. Cloud-locked devices are always forced back to
/// DeepInfra Whisper. The previous Together preference migrates in place so
/// released clients keep their cloud selection after updating.
pub fn normalize_prefs(prefs: DesktopPrefs) -> DesktopPrefs {
    normalize_prefs_for(prefs, current())
}

fn normalize_prefs_for(mut prefs: DesktopPrefs, policy: &SttSetupPolicy) -> DesktopPrefs {
    if policy.is_cloud_locked() {
        prefs.dictation_stt = CLOUD_DEEPINFRA_PREF.to_string();
        prefs.local_stt_compat_override = None;
        return prefs;
    }

    // Apple Silicon offers exactly one user decision in Settings: local or
    // hosted DeepInfra Whisper. Preserve the previous Together cloud choice
    // during migration; every other legacy value becomes the local default.
    if prefs.dictation_stt == LEGACY_CLOUD_NEMOTRON_PREF {
        prefs.dictation_stt = CLOUD_DEEPINFRA_PREF.to_string();
    } else if prefs.dictation_stt != CLOUD_DEEPINFRA_PREF {
        prefs.dictation_stt = LOCAL_PREF.to_string();
    }

    // There is one local model now, so there is nothing to choose between and
    // no compatibility escape hatch to honour. Any stored model name — Oriserve,
    // either Nemotron, or a compatibility override written by an older release —
    // collapses to the current model.
    prefs.local_stt_compat_override = None;
    if let Some(model) = policy.local_pref() {
        prefs.local_stt_model = model.to_string();
    }
    prefs
}

/// Persist normalization at startup, before onboarding, Settings, or a hotkey
/// can read a stale preference from an older release.
pub fn normalize_persisted_prefs() -> Result<DesktopPrefs, String> {
    let before = said_core::prefs::load();
    let after = normalize_prefs(before.clone());
    if before.dictation_stt != after.dictation_stt
        || before.local_stt_model != after.local_stt_model
        || before.local_stt_compat_override != after.local_stt_compat_override
    {
        said_core::prefs::save(&after)?;
        tracing::info!(
            platform = %after_platform(),
            dictation_stt = %after.dictation_stt,
            local_stt_model = %after.local_stt_model,
            "[stt-policy] normalized desktop speech preferences"
        );
    }
    Ok(after)
}

fn after_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

fn detect() -> SttSetupPolicy {
    let mut system = System::new();
    system.refresh_memory();
    let architecture = System::cpu_arch().unwrap_or_else(|| "unknown".to_string());
    policy_for(
        after_platform(),
        &architecture,
        macos_rosetta_translated(),
        system.total_memory(),
    )
}

/// Kept pure so macOS/Windows policy outcomes are fully testable from any host.
pub(crate) fn policy_for(
    platform: &str,
    architecture: &str,
    rosetta_translated: bool,
    total_memory_bytes: u64,
) -> SttSetupPolicy {
    let apple_silicon = platform == "macos"
        && (matches!(
            architecture.to_ascii_lowercase().as_str(),
            "arm64" | "aarch64"
        ) || rosetta_translated);

    if apple_silicon {
        // One model for every Apple Silicon Mac. The previous 8 GB / above-8 GB
        // split existed because Nemotron Q4 was too heavy for small machines;
        // the current model is a ~141 MB whisper-base fine-tune, comfortable on
        // the smallest supported Mac, so the tiering has no reason to exist.
        return SttSetupPolicy {
            platform: platform.to_string(),
            cpu_family: "apple_silicon".to_string(),
            total_memory_bytes,
            setup_kind: SetupKind::LocalRequired,
            local_model: Some(CLARIO_41H_PREF.to_string()),
            local_model_name: Some("AirNote Hinglish (41h)".to_string()),
            local_model_size_hint: Some("~141 MB".to_string()),
        };
    }

    let cpu_family = if platform == "macos" {
        "intel"
    } else {
        "windows_or_other"
    };
    SttSetupPolicy {
        platform: platform.to_string(),
        cpu_family: cpu_family.to_string(),
        total_memory_bytes,
        setup_kind: SetupKind::CloudLocked,
        local_model: None,
        local_model_name: None,
        local_model_size_hint: None,
    }
}

/// True when the process is an Intel binary translated by Rosetta on an Apple
/// Silicon Mac. Jan checks runtime CPU architecture; this additional macOS
/// probe keeps the result correct for a translated compatibility build too.
#[cfg(target_os = "macos")]
fn macos_rosetta_translated() -> bool {
    std::process::Command::new("sysctl")
        .args(["-in", "sysctl.proc_translated"])
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout == b"1\n")
}

#[cfg(not(target_os = "macos"))]
fn macos_rosetta_translated() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const EIGHT_GIB: u64 = 8 * 1024 * 1024 * 1024;

    #[test]
    fn windows_is_always_hosted_deepinfra() {
        let policy = policy_for("windows", "x86_64", false, 64 * EIGHT_GIB);
        assert!(policy.is_cloud_locked());
        assert_eq!(policy.local_model, None);
    }

    #[test]
    fn intel_mac_is_always_hosted_deepinfra() {
        let policy = policy_for("macos", "x86_64", false, 64 * EIGHT_GIB);
        assert!(policy.is_cloud_locked());
        assert_eq!(policy.cpu_family, "intel");
    }

    #[test]
    fn rosetta_is_still_classified_as_apple_silicon() {
        let policy = policy_for("macos", "x86_64", true, 16 * 1024 * 1024 * 1024);
        assert_eq!(policy.cpu_family, "apple_silicon");
        assert_eq!(policy.local_model.as_deref(), Some(CLARIO_41H_PREF));
    }

    /// The memory tiers are gone. An 8 GB Mac and a 64 GB Mac now receive the
    /// same model, which is the whole point of the single-model release.
    #[test]
    fn every_apple_silicon_mac_gets_the_same_model_regardless_of_memory() {
        for memory in [4 * EIGHT_GIB / 8, EIGHT_GIB, EIGHT_GIB + 1, 64 * EIGHT_GIB] {
            let policy = policy_for("macos", "arm64", false, memory);
            assert_eq!(
                policy.local_model.as_deref(),
                Some(CLARIO_41H_PREF),
                "memory {memory} should not change the model"
            );
            assert_eq!(policy.setup_kind, SetupKind::LocalRequired);
        }
    }

    #[test]
    fn cloud_locked_policy_overrides_every_stale_preference() {
        let policy = policy_for("windows", "x86_64", false, EIGHT_GIB);
        let normalized = normalize_prefs_for(
            DesktopPrefs {
                dictation_stt: "local".into(),
                local_stt_model: NEMOTRON_Q4_PREF.into(),
                ..DesktopPrefs::default()
            },
            &policy,
        );
        assert_eq!(normalized.dictation_stt, CLOUD_DEEPINFRA_PREF);
        assert_eq!(normalized.local_stt_compat_override, None);
    }

    #[test]
    fn apple_silicon_keeps_explicit_deepinfra_and_migrates_together() {
        let policy = policy_for("macos", "arm64", false, EIGHT_GIB + 1);
        let local = normalize_prefs_for(DesktopPrefs::default(), &policy);
        assert_eq!(local.dictation_stt, LOCAL_PREF);
        assert_eq!(local.local_stt_model, CLARIO_41H_PREF);

        let cloud = normalize_prefs_for(
            DesktopPrefs {
                dictation_stt: CLOUD_DEEPINFRA_PREF.into(),
                ..DesktopPrefs::default()
            },
            &policy,
        );
        assert_eq!(cloud.dictation_stt, CLOUD_DEEPINFRA_PREF);
    }

    /// Every existing user is upgrading from a release that stored Oriserve,
    /// Nemotron Q4/Q8, or a compatibility override. All of them must land on
    /// the current model rather than a name nothing can download any more.
    #[test]
    fn every_retired_model_selection_migrates_to_the_current_one() {
        let policy = policy_for("macos", "arm64", false, EIGHT_GIB);
        for stored in [
            ORISERVE_PREF,
            NEMOTRON_Q4_PREF,
            "nemotron-q8",
            "nemotron",
            "",
        ] {
            let normalized = normalize_prefs_for(
                DesktopPrefs {
                    dictation_stt: LOCAL_PREF.into(),
                    local_stt_model: stored.into(),
                    local_stt_compat_override: Some(ORISERVE_PREF.into()),
                    ..DesktopPrefs::default()
                },
                &policy,
            );
            assert_eq!(
                normalized.local_stt_model, CLARIO_41H_PREF,
                "stored selection {stored:?} should migrate"
            );
            assert_eq!(
                normalized.local_stt_compat_override, None,
                "compatibility overrides no longer exist"
            );
        }
    }
}
