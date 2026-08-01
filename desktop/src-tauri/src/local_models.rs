//! Inventory and migration-safe lifecycle for local speech models.
//!
//! Catalog-backed dictation models are generic. Oriserve remains a protected
//! meeting dependency until that legacy Whisper artifact joins the verified
//! catalog installer.

use serde::Serialize;

use said_core::prefs::DesktopPrefs;

use crate::{
    dictation_stt, local_model_catalog, local_model_store, local_transcribe, meeting_engine,
    stt_policy,
};

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
    pub selectable: bool,
    pub safe_to_remove: bool,
    pub architecture: String,
    pub languages: Vec<String>,
    pub streaming: bool,
    pub quantization: Option<String>,
    pub license: Option<String>,
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

fn canonical_local_model(value: &str) -> &str {
    local_model_catalog::canonical_key(value)
}

fn status_for(key: &str) -> Result<(bool, u64), String> {
    if key == stt_policy::ORISERVE_PREF {
        let status = meeting_engine::dictation_model_status();
        return Ok((status.installed, status.size_bytes));
    }
    let descriptor = local_model_catalog::find(key)
        .ok_or_else(|| format!("Unsupported local speech model: {key}"))?;
    Ok((
        local_model_store::installed(descriptor),
        local_model_store::installed_size(descriptor),
    ))
}

fn model_flags(
    key: &str,
    installed: bool,
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
) -> (bool, bool, bool, bool, bool) {
    let recommended = policy.local_pref() == Some(key);
    let active = prefs.dictation_stt == stt_policy::LOCAL_PREF
        && canonical_local_model(&prefs.local_stt_model) == key;
    let selectable = stt_policy::supports_local_model(policy, key);
    let compatible = installed && selectable;
    let safe_to_remove = installed && key != stt_policy::ORISERVE_PREF && !active && !recommended;
    (recommended, active, compatible, selectable, safe_to_remove)
}

fn oriserve_info(
    status: (bool, u64),
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
) -> LocalModelInfo {
    let (installed, size_bytes) = status;
    let (recommended, active, compatible, selectable, safe_to_remove) =
        model_flags(stt_policy::ORISERVE_PREF, installed, policy, prefs);
    LocalModelInfo {
        key: stt_policy::ORISERVE_PREF.into(),
        name: "Oriserve Hinglish".into(),
        installed,
        size_bytes,
        size_hint: "~148 MB".into(),
        recommended,
        active_for_dictation: active,
        required_for_meetings: true,
        compatibility_candidate: compatible,
        selectable,
        safe_to_remove,
        architecture: "whisper".into(),
        languages: vec!["en".into(), "hi".into()],
        streaming: false,
        quantization: None,
        license: None,
    }
}

fn catalog_info(
    descriptor: &local_model_catalog::LocalModelDescriptor,
    status: (bool, u64),
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
) -> LocalModelInfo {
    let (installed, size_bytes) = status;
    let (recommended, active, compatible, selectable, safe_to_remove) =
        model_flags(descriptor.key, installed, policy, prefs);
    LocalModelInfo {
        key: descriptor.key.into(),
        name: descriptor.name.into(),
        installed,
        size_bytes,
        size_hint: descriptor.size_hint(),
        recommended,
        active_for_dictation: active,
        required_for_meetings: false,
        compatibility_candidate: compatible,
        selectable,
        safe_to_remove,
        architecture: descriptor.architecture.into(),
        languages: descriptor
            .languages
            .iter()
            .map(|value| (*value).into())
            .collect(),
        streaming: descriptor.capabilities.streaming,
        quantization: Some(descriptor.quantization.into()),
        license: Some(descriptor.license.into()),
    }
}

fn inventory_for(
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
    statuses: &[(String, bool, u64)],
) -> LocalModelInventory {
    let status = |key: &str| {
        statuses
            .iter()
            .find(|(candidate, _, _)| candidate == key)
            .map(|(_, installed, size)| (*installed, *size))
            .unwrap_or((false, 0))
    };
    let mut models = vec![oriserve_info(
        status(stt_policy::ORISERVE_PREF),
        policy,
        prefs,
    )];
    models.extend(
        local_model_catalog::MODELS
            .iter()
            .map(|descriptor| catalog_info(descriptor, status(descriptor.key), policy, prefs)),
    );

    let recommended_installed = models
        .iter()
        .any(|model| model.recommended && model.installed);
    let selected = canonical_local_model(&prefs.local_stt_model);
    let existing_compatible_model = models
        .iter()
        .find(|model| model.key == selected && model.compatibility_candidate && !model.recommended)
        .or_else(|| {
            (!recommended_installed).then(|| {
                models
                    .iter()
                    .find(|model| model.compatibility_candidate && !model.recommended)
            })?
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
    let (oriserve_installed, oriserve_size) = status_for(stt_policy::ORISERVE_PREF)?;
    let mut statuses = vec![(
        stt_policy::ORISERVE_PREF.to_string(),
        oriserve_installed,
        oriserve_size,
    )];
    for descriptor in local_model_catalog::MODELS {
        let (installed, size) = status_for(descriptor.key)?;
        statuses.push((descriptor.key.to_string(), installed, size));
    }
    Ok(inventory_for(policy, &prefs, &statuses))
}

/// Select an installed model only after policy and integrity checks succeed.
#[tauri::command]
pub fn choose_installed_local_model(model: String) -> Result<LocalModelInventory, String> {
    let policy = stt_policy::current();
    if policy.is_cloud_locked() {
        return Err("Local dictation models are not selectable on this device.".into());
    }
    let model = canonical_local_model(&model).to_string();
    if !stt_policy::supports_local_model(policy, &model) {
        return Err("That model is not supported on this device.".into());
    }
    let (installed, _) = status_for(&model)?;
    if !installed {
        return Err("That speech model is not fully installed yet.".into());
    }
    if let Some(descriptor) = local_model_catalog::find(&model) {
        local_model_store::ensure_verified(descriptor)?;
    }

    let recommended = policy
        .local_pref()
        .ok_or_else(|| "This device has no local model recommendation.".to_string())?;
    let compatibility = model != recommended;
    let mut prefs = said_core::prefs::load();
    prefs.dictation_stt = stt_policy::LOCAL_PREF.into();
    prefs.local_stt_model = model.clone();
    prefs.local_stt_compat_override = compatibility.then_some(model);
    let prefs = stt_policy::normalize_prefs(prefs);
    said_core::prefs::save(&prefs)?;
    local_transcribe::unload();
    std::thread::Builder::new()
        .name("dictation-stt-model-prewarm".into())
        .spawn(dictation_stt::prewarm)
        .ok();
    local_model_inventory()
}

#[tauri::command]
pub fn remove_unused_local_dictation_models() -> Result<LocalModelCleanupResult, String> {
    let inventory = local_model_inventory()?;
    let mut removed = Vec::new();
    for model in inventory.models.iter().filter(|model| model.safe_to_remove) {
        let descriptor = local_model_catalog::find(&model.key)
            .ok_or_else(|| format!("Unknown catalog model: {}", model.key))?;
        local_model_store::remove(descriptor)?;
        removed.push(RemovedLocalModel {
            key: model.key.clone(),
            name: model.name.clone(),
            size_bytes: model.size_bytes,
        });
    }
    local_transcribe::unload();
    let freed_bytes = removed.iter().map(|model| model.size_bytes).sum();
    Ok(LocalModelCleanupResult {
        removed,
        freed_bytes,
    })
}

/// Explicit advanced reset. Switch to a working cloud route before removing
/// local artifacts. Oriserve and Silero are included, so Meetings require a
/// fresh download afterwards.
#[tauri::command]
pub fn delete_all_local_speech_models() -> Result<LocalModelCleanupResult, String> {
    let previous = said_core::prefs::load();
    let policy = stt_policy::current();
    let mut cloud = previous.clone();
    if cloud.dictation_stt == stt_policy::LOCAL_PREF {
        cloud.dictation_stt = stt_policy::CLOUD_ELEVENLABS_PREF.into();
    }
    cloud.local_stt_compat_override = None;
    if let Some(recommended) = policy.local_pref() {
        cloud.local_stt_model = recommended.into();
    }
    let cloud = stt_policy::normalize_prefs(cloud);
    said_core::prefs::save(&cloud)?;
    if !dictation_stt::dictation_ready() {
        said_core::prefs::save(&previous)?;
        return Err("The selected cloud speech model is not ready, so AirNote kept your local speech models.".into());
    }

    let inventory = local_model_inventory()?;
    let vad = meeting_engine::silero_vad_model_status();
    local_transcribe::unload();
    let mut removed = Vec::new();
    for model in inventory.models.iter().filter(|model| model.installed) {
        if model.key == stt_policy::ORISERVE_PREF {
            meeting_engine::delete_dictation_model()?;
        } else if let Some(descriptor) = local_model_catalog::find(&model.key) {
            local_model_store::remove(descriptor)?;
        } else {
            continue;
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

    fn statuses(values: &[(&str, bool, u64)]) -> Vec<(String, bool, u64)> {
        values
            .iter()
            .map(|(key, installed, size)| ((*key).into(), *installed, *size))
            .collect()
    }

    #[test]
    fn catalog_models_are_exposed_without_provider_specific_inventory_code() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs::default();
        let inventory = inventory_for(
            &policy,
            &prefs,
            &statuses(&[(stt_policy::ORISERVE_PREF, true, 148)]),
        );
        assert!(inventory.models.iter().any(|model| {
            model.key == local_model_catalog::PARAKEET_Q8_PREF
                && model.languages == ["en"]
                && !model.streaming
        }));
    }

    #[test]
    fn meetings_model_is_never_normal_cleanup_candidate() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs {
            local_stt_model: stt_policy::NEMOTRON_Q4_PREF.into(),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(
            &policy,
            &prefs,
            &statuses(&[
                (stt_policy::ORISERVE_PREF, true, 148),
                (stt_policy::NEMOTRON_Q4_PREF, true, 496),
                (local_model_catalog::NEMOTRON_Q8_PREF, true, 751),
            ]),
        );
        let oriserve = &inventory.models[0];
        assert!(oriserve.required_for_meetings);
        assert!(!oriserve.safe_to_remove);
        assert_eq!(inventory.reclaimable_bytes, 751);
    }

    #[test]
    fn selected_parakeet_is_a_first_class_compatibility_choice() {
        let policy = apple_policy(16);
        let prefs = DesktopPrefs {
            local_stt_model: local_model_catalog::PARAKEET_Q8_PREF.into(),
            local_stt_compat_override: Some(local_model_catalog::PARAKEET_Q8_PREF.into()),
            ..DesktopPrefs::default()
        };
        let inventory = inventory_for(
            &policy,
            &prefs,
            &statuses(&[(local_model_catalog::PARAKEET_Q8_PREF, true, 731)]),
        );
        assert_eq!(
            inventory.existing_compatible_model.as_deref(),
            Some(local_model_catalog::PARAKEET_Q8_PREF)
        );
    }
}
