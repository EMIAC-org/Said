//! Inventory and migration-safe lifecycle for local speech models.
//!
//! AirNote ships exactly one local dictation model now. Everything else in here
//! exists to recognise what older releases left on disk and offer to reclaim it.
//!
//! One subtlety drives the shape of this file: the current model and the
//! retired Oriserve model occupy the *same* path, because the STT runtime loads
//! a fixed filename. Their sizes are close enough that bytes cannot tell them
//! apart, so `dictation_model`'s installed-marker is the only reliable signal.
//! A marker means the current model; a bare file means the retired one.

use serde::Serialize;

use said_core::prefs::DesktopPrefs;

use crate::{dictation_model, dictation_stt, meeting_engine, nemotron, stt_policy};

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

/// Is this model installed, and how much disk does it hold?
///
/// Oriserve and the current model share a path, so both are resolved through
/// the marker rather than by looking at the file alone.
fn status_for(key: &str) -> Result<(bool, u64), String> {
    match key {
        stt_policy::CLARIO_41H_PREF => {
            // `installed` already requires the marker: a bare file is the
            // pre-release Oriserve model sitting at this path.
            let status = meeting_engine::dictation_model_status();
            let is_current = status.installed;
            Ok((is_current, if is_current { status.size_bytes } else { 0 }))
        }
        stt_policy::ORISERVE_PREF => {
            let status = meeting_engine::dictation_model_status();
            let is_legacy = meeting_engine::dictation_model_file_present()
                && dictation_model::read_marker().is_none();
            Ok((is_legacy, if is_legacy { status.size_bytes } else { 0 }))
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
    let active_for_dictation = prefs.dictation_stt == stt_policy::LOCAL_PREF && recommended;
    // Retired Nemotron variants are genuinely dead weight: separate files that
    // nothing loads any more. Oriserve is different — it sits at the *same*
    // path the current model installs to, so it is not reclaimable disk, it is
    // the old model awaiting replacement. Offering to delete it would leave the
    // machine with no dictation model at all; the refresh flow overwrites it
    // instead.
    let safe_to_remove = installed && !recommended && key != stt_policy::ORISERVE_PREF;
    LocalModelInfo {
        key: key.into(),
        name: name.into(),
        installed,
        size_bytes,
        size_hint: size_hint.into(),
        recommended,
        active_for_dictation,
        // Meetings has been removed from the product, so nothing is pinned by it.
        required_for_meetings: false,
        // There is one model; there is nothing to fall back to.
        compatibility_candidate: false,
        safe_to_remove,
    }
}

/// Collapse any stored selection onto the current model. Older releases wrote
/// Oriserve, either Nemotron variant, or a bare "nemotron"; none of those can
/// be downloaded any more, so treating them as the current model is what keeps
/// an upgraded install working instead of pointing at a model that is gone.
fn canonical_local_model(_value: &str) -> &'static str {
    stt_policy::CLARIO_41H_PREF
}

fn inventory_for(
    policy: &stt_policy::SttSetupPolicy,
    prefs: &DesktopPrefs,
    statuses: [(bool, u64); 4],
) -> LocalModelInventory {
    let [
        (current_installed, current_size),
        (oriserve_installed, oriserve_size),
        (q4_installed, q4_size),
        (q8_installed, q8_size),
    ] = statuses;

    // The current model leads; the rest appear only so the UI can offer to
    // reclaim their disk. Retired entries are hidden once they are gone.
    let mut models = vec![model_info(
        stt_policy::CLARIO_41H_PREF,
        "AirNote Hinglish (41h)",
        "~141 MB",
        current_installed,
        current_size,
        policy,
        prefs,
    )];
    for (key, name, hint, installed, size) in [
        (
            stt_policy::ORISERVE_PREF,
            "Oriserve Hinglish (retired)",
            "~148 MB",
            oriserve_installed,
            oriserve_size,
        ),
        (
            stt_policy::NEMOTRON_Q4_PREF,
            "Nemotron Streaming 3.5 Q4 (retired)",
            "~496 MB",
            q4_installed,
            q4_size,
        ),
        (
            NEMOTRON_Q8_PREF,
            "Nemotron Streaming 3.5 Q8 (retired)",
            "~751 MB",
            q8_installed,
            q8_size,
        ),
    ] {
        if installed {
            models.push(model_info(key, name, hint, installed, size, policy, prefs));
        }
    }

    let recommended_installed = models
        .iter()
        .any(|model| model.recommended && model.installed);
    let reclaimable_bytes = models
        .iter()
        .filter(|model| model.safe_to_remove)
        .map(|model| model.size_bytes)
        .sum();
    LocalModelInventory {
        setup_kind: policy.setup_kind,
        recommended_model: policy.local_model.clone(),
        selected_model: canonical_local_model(&prefs.local_stt_model).into(),
        recommended_installed,
        // Nothing to fall back to any more: one model, or none.
        existing_compatible_model: None,
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
            status_for(stt_policy::CLARIO_41H_PREF)?,
            status_for(stt_policy::ORISERVE_PREF)?,
            status_for(stt_policy::NEMOTRON_Q4_PREF)?,
            status_for(NEMOTRON_Q8_PREF)?,
        ],
    ))
}

/// Switch dictation to the local model.
///
/// There is one local model now, so this no longer picks between candidates —
/// it confirms the model is actually installed and records the choice. The
/// `model` argument is kept because the frontend still sends one, and because
/// an upgraded install will send a retired name that has to be accepted rather
/// than rejected.
#[tauri::command]
pub fn choose_installed_local_model(model: String) -> Result<LocalModelInventory, String> {
    let policy = stt_policy::current();
    if policy.is_cloud_locked() {
        return Err("Local dictation models are not selectable on this device.".into());
    }
    let model = canonical_local_model(&model).to_string();
    let (installed, _) = status_for(&model)?;
    if !installed {
        return Err("The speech model is not fully installed yet.".into());
    }

    let mut prefs = said_core::prefs::load();
    prefs.dictation_stt = stt_policy::LOCAL_PREF.into();
    prefs.local_stt_model = model;
    prefs.local_stt_compat_override = None;
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
        // `safe_to_remove` only ever marks Nemotron variants (see `model_info`),
        // but match explicitly so a future addition cannot silently be routed
        // into the Nemotron deleter and delete the wrong file.
        let variant = match model.key.as_str() {
            stt_policy::NEMOTRON_Q4_PREF => "q4",
            NEMOTRON_Q8_PREF => "q8",
            other => {
                tracing::warn!("[local-models] refusing to reclaim unexpected model {other}");
                continue;
            }
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

    const EIGHT_GIB: u64 = 8 * 1024 * 1024 * 1024;

    fn apple_silicon() -> stt_policy::SttSetupPolicy {
        stt_policy::policy_for("macos", "arm64", false, EIGHT_GIB)
    }

    fn local_prefs() -> DesktopPrefs {
        DesktopPrefs {
            dictation_stt: stt_policy::LOCAL_PREF.into(),
            local_stt_model: stt_policy::CLARIO_41H_PREF.into(),
            ..DesktopPrefs::default()
        }
    }

    /// Nothing installed at all — a clean machine.
    fn nothing_installed() -> [(bool, u64); 4] {
        [(false, 0), (false, 0), (false, 0), (false, 0)]
    }

    #[test]
    fn a_clean_machine_lists_only_the_current_model() {
        let inventory = inventory_for(&apple_silicon(), &local_prefs(), nothing_installed());
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].key, stt_policy::CLARIO_41H_PREF);
        assert!(!inventory.recommended_installed);
        assert_eq!(inventory.reclaimable_bytes, 0);
    }

    #[test]
    fn retired_models_appear_only_while_they_are_still_on_disk() {
        let mut statuses = nothing_installed();
        statuses[0] = (true, 141); // current
        statuses[2] = (true, 496); // Nemotron Q4 left over
        let inventory = inventory_for(&apple_silicon(), &local_prefs(), statuses);

        let keys: Vec<_> = inventory.models.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![stt_policy::CLARIO_41H_PREF, stt_policy::NEMOTRON_Q4_PREF]
        );
        // Oriserve and Q8 are absent because they are not installed.
        assert!(!keys.contains(&stt_policy::ORISERVE_PREF));
    }

    #[test]
    fn the_current_model_is_never_offered_for_removal() {
        let mut statuses = nothing_installed();
        statuses[0] = (true, 141);
        let inventory = inventory_for(&apple_silicon(), &local_prefs(), statuses);
        let current = &inventory.models[0];
        assert!(current.recommended);
        assert!(current.active_for_dictation);
        assert!(!current.safe_to_remove);
        assert_eq!(inventory.reclaimable_bytes, 0);
    }

    #[test]
    fn every_retired_model_is_reclaimable_and_their_bytes_add_up() {
        let statuses = [(true, 141), (true, 148), (true, 496), (true, 751)];
        let inventory = inventory_for(&apple_silicon(), &local_prefs(), statuses);

        let reclaimable: Vec<_> = inventory
            .models
            .iter()
            .filter(|m| m.safe_to_remove)
            .map(|m| m.key.as_str())
            .collect();
        // Oriserve is excluded on purpose: it shares a path with the current
        // model, so it is replaced rather than reclaimed.
        assert_eq!(
            reclaimable,
            vec![stt_policy::NEMOTRON_Q4_PREF, NEMOTRON_Q8_PREF]
        );
        assert_eq!(inventory.reclaimable_bytes, 496 + 751);
    }

    /// Meetings is gone from the product, so no model is pinned by it. The old
    /// behaviour protected Oriserve from removal for exactly that reason.
    #[test]
    fn nothing_is_pinned_by_meetings_any_more() {
        let statuses = [(true, 141), (true, 148), (false, 0), (false, 0)];
        let inventory = inventory_for(&apple_silicon(), &local_prefs(), statuses);
        assert!(inventory.models.iter().all(|m| !m.required_for_meetings));

        let oriserve = inventory
            .models
            .iter()
            .find(|m| m.key == stt_policy::ORISERVE_PREF)
            .expect("retired Oriserve should be listed while installed");
        // Not reclaimable: deleting it would strand the machine with no model.
        assert!(!oriserve.safe_to_remove);
    }

    /// Whatever an older release stored, the app has to resolve it to the model
    /// it can actually download today.
    #[test]
    fn any_stored_selection_resolves_to_the_current_model() {
        for stored in [
            stt_policy::ORISERVE_PREF,
            stt_policy::NEMOTRON_Q4_PREF,
            "nemotron",
            "nemotron-q8",
            "",
        ] {
            assert_eq!(canonical_local_model(stored), stt_policy::CLARIO_41H_PREF);
        }
    }

    #[test]
    fn a_cloud_locked_machine_recommends_no_local_model() {
        let policy = stt_policy::policy_for("windows", "x86_64", false, EIGHT_GIB);
        let inventory = inventory_for(&policy, &DesktopPrefs::default(), nothing_installed());
        assert_eq!(inventory.recommended_model, None);
        assert!(inventory.models.iter().all(|m| !m.recommended));
    }
}
