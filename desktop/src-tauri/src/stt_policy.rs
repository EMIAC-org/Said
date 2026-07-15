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

pub const CLOUD_NEMOTRON_PREF: &str = "cloud-nemotron-3.5";
pub const LOCAL_PREF: &str = "local";
pub const ORISERVE_PREF: &str = "oriserve";
pub const NEMOTRON_Q4_PREF: &str = "nemotron-q4";

const EIGHT_GIB: u64 = 8 * 1024 * 1024 * 1024;

/// What AirNote can expose for dictation on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupKind {
    /// Windows and Intel Macs always use the live Together Nemotron service.
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
/// live Nemotron; an old `auto`, local, or Whisper selection cannot revive a
/// retired route.
pub fn normalize_prefs(prefs: DesktopPrefs) -> DesktopPrefs {
    normalize_prefs_for(prefs, current())
}

fn normalize_prefs_for(mut prefs: DesktopPrefs, policy: &SttSetupPolicy) -> DesktopPrefs {
    if policy.is_cloud_locked() {
        prefs.dictation_stt = CLOUD_NEMOTRON_PREF.to_string();
        return prefs;
    }

    // Apple Silicon offers exactly one user decision in Settings: local or
    // cloud Nemotron. Every legacy value, including cloud Whisper, becomes the
    // cost-safe local default.
    if prefs.dictation_stt != CLOUD_NEMOTRON_PREF {
        prefs.dictation_stt = LOCAL_PREF.to_string();
    }
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
fn policy_for(
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
        // An 8 GB M-series Mac always receives Oriserve. Q4 starts strictly
        // above that threshold, per the product policy.
        let q4 = total_memory_bytes > EIGHT_GIB;
        return SttSetupPolicy {
            platform: platform.to_string(),
            cpu_family: "apple_silicon".to_string(),
            total_memory_bytes,
            setup_kind: SetupKind::LocalRequired,
            local_model: Some(if q4 { NEMOTRON_Q4_PREF } else { ORISERVE_PREF }.to_string()),
            local_model_name: Some(
                if q4 {
                    "Nemotron Streaming 3.5 (Q4)"
                } else {
                    "Oriserve Hinglish"
                }
                .to_string(),
            ),
            local_model_size_hint: Some(if q4 { "~496 MB" } else { "~148 MB" }.to_string()),
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

    #[test]
    fn windows_is_always_live_nemotron() {
        let policy = policy_for("windows", "x86_64", false, 64 * EIGHT_GIB);
        assert!(policy.is_cloud_locked());
        assert_eq!(policy.local_model, None);
    }

    #[test]
    fn intel_mac_is_always_live_nemotron() {
        let policy = policy_for("macos", "x86_64", false, 64 * EIGHT_GIB);
        assert!(policy.is_cloud_locked());
        assert_eq!(policy.cpu_family, "intel");
    }

    #[test]
    fn rosetta_is_still_classified_as_apple_silicon() {
        let policy = policy_for("macos", "x86_64", true, 16 * 1024 * 1024 * 1024);
        assert_eq!(policy.cpu_family, "apple_silicon");
        assert_eq!(policy.local_model.as_deref(), Some(NEMOTRON_Q4_PREF));
    }

    #[test]
    fn eight_gib_apple_silicon_uses_oriserve() {
        let policy = policy_for("macos", "arm64", false, EIGHT_GIB);
        assert_eq!(policy.local_model.as_deref(), Some(ORISERVE_PREF));
    }

    #[test]
    fn more_than_eight_gib_apple_silicon_uses_q4() {
        let policy = policy_for("macos", "aarch64", false, EIGHT_GIB + 1);
        assert_eq!(policy.local_model.as_deref(), Some(NEMOTRON_Q4_PREF));
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
        assert_eq!(normalized.dictation_stt, CLOUD_NEMOTRON_PREF);
    }

    #[test]
    fn apple_silicon_keeps_only_an_explicit_cloud_nemotron_choice() {
        let policy = policy_for("macos", "arm64", false, EIGHT_GIB + 1);
        let local = normalize_prefs_for(DesktopPrefs::default(), &policy);
        assert_eq!(local.dictation_stt, LOCAL_PREF);
        assert_eq!(local.local_stt_model, NEMOTRON_Q4_PREF);

        let cloud = normalize_prefs_for(
            DesktopPrefs {
                dictation_stt: CLOUD_NEMOTRON_PREF.into(),
                ..DesktopPrefs::default()
            },
            &policy,
        );
        assert_eq!(cloud.dictation_stt, CLOUD_NEMOTRON_PREF);
        assert_eq!(cloud.local_stt_model, NEMOTRON_Q4_PREF);
    }
}
