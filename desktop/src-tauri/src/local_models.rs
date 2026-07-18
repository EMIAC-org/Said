//! Inventory and migration-safe lifecycle for local speech models.
//!
//! Hardware recommendation, active dictation choice, and Meetings dependencies
//! are deliberately separate concepts here. Normal upgrades may remove only
//! unused Nemotron variants; Oriserve remains protected because Meetings use it
//! on every platform.

use serde::Serialize;

use said_core::prefs::DesktopPrefs;

use crate::{dictation_stt, meeting_engine, nemotron, stt_policy};

const NEMOTRON_Q8_PREF: &str = "nemotron-q8";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalModelInfo {
    pub key: String,
    pub name: String,
    pub installed: bool,
    pub size_bytes: u64,
    pub size_hint: String,
    pub recommended: bool,
    pub active_for_dictation: bool,
    pub required_for_meetings: bool,
    pub compatibility_candidate: bool,
    pub safe_to_remove: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalModelInventory {
    pub setup_kind: stt_policy::SetupKind,
    pub recommended_model: Option<String>,
    pub selected_model: String,
    pub recommended_installed: bool,
    pub existing_compatible_model: Option<String>,
    pub models: Vec<LocalModelInfo>,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemovedLocalModel {
    pub key: String,
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalModelCleanupResult {
    pub removed: Vec<RemovedLocalModel>,
    pub freed_bytes: u64,
}

fn status_for(key: &str) -> Result<(bool, u64), String> {
    match key {
        stt_policy::ORISERVE_PREF => {
            let status = meeting_engine::dictation_model_status();
            Ok((status.installed, status.size_bytes))
        }
        stt_policy::NEMOTRON_Q4_PREF => {
            let status = nemotron::nemotron_model_status("q4".into())?;
            Ok((status.installed, status.size_bytes))
        }
        NEMOTRON_Q8_PREF => {
            let status = nemotron::nemotron_model_status("q8".into())?;
            Ok((status.installed, status.size_bytes))
        }
        _ => Err(format!("Unsupported local speech model: {key}")),
    }
}

fn model_info(
    key: &str,
    name: &str,
    size_hint: &str,
    installed: bool,
    size_bytes: u64,
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
) -> LocalModelInfo {
    let recommended = policy.local_pref() == Some(key);
    let active_for_dictation = prefs.dictation_stt == stt_policy::LOCAL_PREF
        && canonical_local_model(&prefs.local_stt_model) == key;
    let compatibility_candidate = policy.local_pref() == Some(stt_policy::NEMOTRON_Q4_PREF)
        && matches!(key, stt_policy::ORISERVE_PREF | NEMOTRON_Q8_PREF)
        && installed;
    let safe_to_remove = installed
        && matches!(key, stt_policy::NEMOTRON_Q4_PREF | NEMOTRON_Q8_PREF)
        && !active_for_dictation
        && !recommended;
    LocalModelInfo {
        key: key.into(),
        name: name.into(),
        installed,
        size_bytes,
        size_hint: size_hint.into(),
        recommended,
        active_for_dictation,
        required_for_meetings: key == stt_policy::ORISERVE_PREF,
        compatibility_candidate,
        safe_to_remove,
    }
}

fn canonical_local_model(value: &str) -> &str {
    match value {
        "nemotron" => NEMOTRON_Q8_PREF,
        other => other,
    }
}

fn inventory_for(
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
    statuses: [(bool, u64); 3],
) -> LocalModelInventory {
    let [
        (oriserve_installed, oriserve_size),
        (q4_installed, q4_size),
        (q8_installed, q8_size),
    ] = statuses;
    let models = vec![
        model_info(
            stt_policy::ORISERVE_PREF,
            "Oriserve Hinglish",
            "~148 MB",
            oriserve_installed,
            oriserve_size,
            policy,
            prefs,
        ),
        model_info(
            stt_policy::NEMOTRON_Q4_PREF,
            "Nemotron Streaming 3.5 (Q4)",
            "~496 MB",
            q4_installed,
            q4_size,
            policy,
            prefs,
        ),
        model_info(
            NEMOTRON_Q8_PREF,
            "Nemotron Streaming 3.5 (Q8)",
            "~751 MB",
            q8_installed,
            q8_size,
            policy,
            prefs,
        ),
    ];
    let recommended_installed = models
        .iter()
        .any(|model| model.recommended && model.installed);
    let selected = canonical_local_model(&prefs.local_stt_model);
    let selected_compatibility = models
        .iter()
        .find(|model| model.key == selected && model.compatibility_candidate);
    let existing_compatible_model = selected_compatibility
        .or_else(|| {
            if recommended_installed {
                None
            } else {
                models
                    .iter()
                    .find(|model| model.key == NEMOTRON_Q8_PREF && model.compatibility_candidate)
            }
        })
        .or_else(|| {
            if recommended_installed {
                None
            } else {
                models.iter().find(|model| model.compatibility_candidate)
            }
        })
        .map(|model| model.key.clone());
    let reclaimable_bytes = models
        .iter()
        .filter(|model| model.safe_to_remove)
        .map(|model| model.size_bytes)
        .sum();
    LocalModelInventory {
        setup_kind: policy.setup_kind,
        recommended_model: policy.local_model.clone(),
        selected_model: selected.into(),
        recommended_installed,
        existing_compatible_model,
        models,
        reclaimable_bytes,
    }
}

#[tauri::command]
pub fn local_model_inventory() -> Result<LocalModelInventory, String> {
    let policy = stt_policy::current();
    let prefs = stt_policy::normalize_prefs(said_core::prefs::load());
    Ok(inventory_for(
        policy,
        &prefs,
        [
            status_for(stt_policy::ORISERVE_PREF)?,
            status_for(stt_policy::NEMOTRON_Q4_PREF)?,
            status_for(NEMOTRON_Q8_PREF)?,
        ],
    ))
}

/// Select a verified installed model. Choosing the hardware recommendation
/// clears compatibility intent; choosing an older supported model records the
/// explicit decision so startup normalization cannot erase it.
#[tauri::command]
pub fn choose_installed_local_model(model: String) -> Result<LocalModelInventory, String> {
    let policy = stt_policy::current();
    if policy.is_cloud_locked() {
        return Err("Local dictation models are not selectable on this device.".into());
    }
    let model = canonical_local_model(&model).to_string();
    let (installed, _) = status_for(&model)?;
    if !installed {
        return Err("That speech model is not fully installed yet.".into());
    }

    let recommended = policy
        .local_pref()
        .ok_or_else(|| "This device has no local model recommendation.".to_string())?;
    let compatibility = recommended == stt_policy::NEMOTRON_Q4_PREF
        && matches!(model.as_str(), stt_policy::ORISERVE_PREF | NEMOTRON_Q8_PREF);
    if model != recommended && !compatibility {
        return Err("That model is not supported for local dictation on this device.".into());
    }

    let mut prefs = said_core::prefs::load();
    prefs.dictation_stt = stt_policy::LOCAL_PREF.into();
    prefs.local_stt_model = model.clone();
    prefs.local_stt_compat_override = compatibility.then_some(model);
    let prefs = stt_policy::normalize_prefs(prefs);
    said_core::prefs::save(&prefs)?;
    std::thread::Builder::new()
        .name("dictation-stt-migration-prewarm".into())
        .spawn(dictation_stt::prewarm)
        .ok();
    local_model_inventory()
}

#[tauri::command]
pub fn remove_unused_local_dictation_models() -> Result<LocalModelCleanupResult, String> {
    let inventory = local_model_inventory()?;
    let mut removed = Vec::new();
    for model in inventory.models.iter().filter(|model| model.safe_to_remove) {
        let variant = if model.key == stt_policy::NEMOTRON_Q4_PREF {
            "q4"
        } else {
            "q8"
        };
        nemotron::delete_nemotron_model(variant.into())?;
        removed.push(RemovedLocalModel {
            key: model.key.clone(),
            name: model.name.clone(),
            size_bytes: model.size_bytes,
        });
    }
    let freed_bytes = removed.iter().map(|model| model.size_bytes).sum();
    Ok(LocalModelCleanupResult {
        removed,
        freed_bytes,
    })
}

/// Explicit advanced reset. Dictation is switched to a verified cloud route
/// before any file is removed. Oriserve and Silero are included, so local
/// Meetings will require a fresh download afterwards.
#[tauri::command]
pub fn delete_all_local_speech_models() -> Result<LocalModelCleanupResult, String> {
    let previous = said_core::prefs::load();
    let policy = stt_policy::current();
    let mut cloud = previous.clone();
    cloud.dictation_stt = stt_policy::CLOUD_DEEPINFRA_PREF.into();
    cloud.local_stt_compat_override = None;
    if let Some(recommended) = policy.local_pref() {
        cloud.local_stt_model = recommended.into();
    }
    let cloud = stt_policy::normalize_prefs(cloud);
    said_core::prefs::save(&cloud)?;
    if !dictation_stt::dictation_ready() {
        said_core::prefs::save(&previous)?;
        return Err("Cloud Whisper is not ready, so AirNote kept your local speech models.".into());
    }

    let inventory = local_model_inventory()?;
    let vad = meeting_engine::silero_vad_model_status();
    nemotron::unload();
    let mut removed = Vec::new();

    for model in inventory.models.iter().filter(|model| model.installed) {
        match model.key.as_str() {
            stt_policy::ORISERVE_PREF => meeting_engine::delete_dictation_model()?,
            stt_policy::NEMOTRON_Q4_PREF => nemotron::delete_nemotron_model("q4".into())?,
            NEMOTRON_Q8_PREF => nemotron::delete_nemotron_model("q8".into())?,
            _ => continue,
        }
        removed.push(RemovedLocalModel {
            key: model.key.clone(),
            name: model.name.clone(),
            size_bytes: model.size_bytes,
        });
    }
    if vad.installed {
        meeting_engine::meeting_delete_silero_vad_model()?;
        removed.push(RemovedLocalModel {
            key: "silero-vad".into(),
            name: "Silero VAD".into(),
            size_bytes: vad.size_bytes,
        });
    }
    let legacy = meeting_engine::reclaim_old_models()?;
    removed.extend(legacy.removed.into_iter().map(|model| RemovedLocalModel {
        key: model.name.clone(),
        name: model.name,
        size_bytes: model.size_bytes,
    }));
    meeting_engine::meeting_ensure_active_model();
    let freed_bytes = removed.iter().map(|model| model.size_bytes).sum();
    Ok(LocalModelCleanupResult {
        removed,
        freed_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apple_policy(memory_gib: u64) -> stt_policy::SttSetupPolicy {
        stt_policy::policy_for("macos", "arm64", false, memory_gib * 1024 * 1024 * 1024)
    }

    #[test]
    fn high_memory_mac_can_continue_with_installed_oriserve() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs::default();
        let inventory = inventory_for(&policy, &prefs, [(true, 148), (false, 0), (false, 0)]);
        assert_eq!(
            inventory.existing_compatible_model.as_deref(),
            Some(stt_policy::ORISERVE_PREF)
        );
        assert!(!inventory.recommended_installed);
    }

    #[test]
    fn q8_is_preferred_as_existing_nemotron_compatibility_model() {
        let policy = apple_policy(16);
        // v6 may already have overwritten the prior selection with the missing
        // recommendation; inventory still recovers the installed Q8 artifact.
        let prefs = DesktopPrefs {
            local_stt_model: stt_policy::NEMOTRON_Q4_PREF.into(),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(&policy, &prefs, [(true, 148), (false, 0), (true, 751)]);
        assert_eq!(
            inventory.existing_compatible_model.as_deref(),
            Some(NEMOTRON_Q8_PREF)
        );
    }

    #[test]
    fn meetings_model_is_never_normal_cleanup_candidate() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs {
            local_stt_model: stt_policy::NEMOTRON_Q4_PREF.into(),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(&policy, &prefs, [(true, 148), (true, 496), (true, 751)]);
        let oriserve = inventory
            .models
            .iter()
            .find(|model| model.key == stt_policy::ORISERVE_PREF)
            .unwrap();
        assert!(oriserve.required_for_meetings);
        assert!(!oriserve.safe_to_remove);
        assert_eq!(inventory.reclaimable_bytes, 751);
    }

    #[test]
    fn eight_gib_mac_does_not_offer_heavy_compatibility_model() {
        let policy = apple_policy(8);
        let prefs = DesktopPrefs::default();
        let inventory = inventory_for(&policy, &prefs, [(false, 0), (true, 496), (true, 751)]);
        assert_eq!(inventory.existing_compatible_model, None);
    }

    #[test]
    fn installed_recommendation_still_reports_selected_compatibility_choice() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs {
            local_stt_model: stt_policy::ORISERVE_PREF.into(),
            local_stt_compat_override: Some(stt_policy::ORISERVE_PREF.into()),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(&policy, &prefs, [(true, 148), (true, 496), (false, 0)]);
        assert!(inventory.recommended_installed);
        assert_eq!(
            inventory.existing_compatible_model.as_deref(),
            Some(stt_policy::ORISERVE_PREF)
        );
    }

    #[test]
    fn cloud_locked_devices_can_reclaim_nemotron_but_not_meetings_model() {
        let policy = stt_policy::policy_for("windows", "x86_64", false, 16 * 1024 * 1024 * 1024);
        let prefs = DesktopPrefs {
            dictation_stt: stt_policy::CLOUD_DEEPINFRA_PREF.into(),
            local_stt_model: stt_policy::NEMOTRON_Q4_PREF.into(),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(&policy, &prefs, [(true, 148), (true, 496), (true, 751)]);
        assert_eq!(inventory.setup_kind, stt_policy::SetupKind::CloudLocked);
        assert_eq!(inventory.reclaimable_bytes, 1_247);
        assert!(!inventory.models[0].safe_to_remove);
        assert!(inventory.models[1].safe_to_remove);
        assert!(inventory.models[2].safe_to_remove);
    }
}
