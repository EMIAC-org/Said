use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use uuid::Uuid;

pub const DEVELOPER_CONTEXT_CAP_CHARS: usize = 8_000;
const FILE_NAME: &str = "developer_context_profiles.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_command_key")]
    pub command_key: String,
    #[serde(default)]
    pub profiles: Vec<DeveloperProjectProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperProjectProfile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub context: String,
    #[serde(default = "default_profile_enabled")]
    pub enabled: bool,
    #[serde(default = "default_source_type")]
    pub source_type: String,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperSettingsResponse {
    pub settings: DeveloperSettings,
    pub warnings: Vec<DeveloperProfileWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperProfileWarning {
    pub profile_id: String,
    pub alias: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperContextMatch {
    pub outcome: String,
    pub label: String,
    pub project: Option<DeveloperMatchedProject>,
    pub candidates: Vec<DeveloperMatchedProject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperMatchedProject {
    pub id: String,
    pub name: String,
    pub context: String,
    pub matched_alias: String,
    pub match_len: usize,
}

impl Default for DeveloperSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            command_key: default_command_key(),
            profiles: Vec::new(),
        }
    }
}

fn default_command_key() -> String {
    "tray".to_string()
}

fn default_profile_enabled() -> bool {
    true
}

fn default_source_type() -> String {
    "manual".to_string()
}

fn settings_path() -> PathBuf {
    said_core::paths::data_dir().join(FILE_NAME)
}

pub fn load_settings() -> DeveloperSettings {
    let path = settings_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return DeveloperSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_settings(mut settings: DeveloperSettings) -> Result<DeveloperSettingsResponse, String> {
    normalize_settings(&mut settings);
    let warnings = validate_settings(&settings)?;
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create developer settings dir: {e}"))?;
    }
    let text = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("serialize developer settings: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write developer settings tmp: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename developer settings tmp: {e}"))?;
    Ok(DeveloperSettingsResponse { settings, warnings })
}

pub fn response_for(settings: DeveloperSettings) -> DeveloperSettingsResponse {
    let mut settings = settings;
    normalize_settings(&mut settings);
    let warnings = validate_settings(&settings).unwrap_or_default();
    DeveloperSettingsResponse { settings, warnings }
}

pub fn match_transcript(transcript: &str, settings: &DeveloperSettings) -> DeveloperContextMatch {
    let normalized = normalize_words(transcript);
    let tokens = normalized
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let padded = format!(" {normalized} ");
    let mut by_profile = BTreeMap::<String, DeveloperMatchedProject>::new();

    for profile in settings.profiles.iter().filter(|p| p.enabled) {
        let aliases = profile_aliases(profile);
        for alias in aliases {
            let alias_norm = normalize_words(&alias);
            if alias_norm.is_empty() {
                continue;
            }
            let whole_match = padded.contains(&format!(" {alias_norm} "));
            let compact_match = compact_alias_matches(&tokens, &alias_norm);
            if !whole_match && !compact_match {
                continue;
            }
            let match_len = alias_norm.chars().filter(|c| c.is_alphanumeric()).count();
            let entry = DeveloperMatchedProject {
                id: profile.id.clone(),
                name: profile.name.clone(),
                context: profile
                    .context
                    .chars()
                    .take(DEVELOPER_CONTEXT_CAP_CHARS)
                    .collect(),
                matched_alias: alias,
                match_len,
            };
            match by_profile.get(&profile.id) {
                Some(existing) if existing.match_len >= entry.match_len => {}
                _ => {
                    by_profile.insert(profile.id.clone(), entry);
                }
            }
        }
    }

    if by_profile.is_empty() {
        return DeveloperContextMatch {
            outcome: "none".to_string(),
            label: "No Project Context".to_string(),
            project: None,
            candidates: Vec::new(),
        };
    }

    let mut candidates = by_profile.into_values().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.match_len.cmp(&a.match_len).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    let best_len = candidates.first().map(|c| c.match_len).unwrap_or(0);
    let best = candidates
        .iter()
        .filter(|c| c.match_len == best_len)
        .cloned()
        .collect::<Vec<_>>();

    if best.len() == 1 {
        let project = best[0].clone();
        return DeveloperContextMatch {
            outcome: "project".to_string(),
            label: format!("Using Context: {}", project.name),
            project: Some(project),
            candidates,
        };
    }

    DeveloperContextMatch {
        outcome: "ambiguous".to_string(),
        label: "Ambiguous Project Match".to_string(),
        project: None,
        candidates,
    }
}

fn normalize_settings(settings: &mut DeveloperSettings) {
    if settings.command_key.trim().is_empty() {
        settings.command_key = default_command_key();
    }
    for profile in &mut settings.profiles {
        if profile.id.trim().is_empty() {
            profile.id = Uuid::new_v4().to_string();
        }
        profile.name = profile.name.trim().to_string();
        profile.aliases = profile
            .aliases
            .iter()
            .map(|alias| alias.trim().to_string())
            .filter(|alias| !alias.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        profile.context = profile.context.trim().to_string();
        if profile.source_type.trim().is_empty() {
            profile.source_type = default_source_type();
        }
        if profile.updated_at == 0 {
            profile.updated_at = now_ms();
        }
    }
}

fn validate_settings(settings: &DeveloperSettings) -> Result<Vec<DeveloperProfileWarning>, String> {
    let mut warnings = Vec::new();
    let mut seen_aliases = BTreeMap::<String, (String, String)>::new();
    for profile in &settings.profiles {
        if profile.name.trim().is_empty() {
            return Err("project name is required".to_string());
        }
        if profile.context.chars().count() > DEVELOPER_CONTEXT_CAP_CHARS {
            return Err(format!(
                "{} context must be at most {} characters",
                profile.name, DEVELOPER_CONTEXT_CAP_CHARS
            ));
        }
        for alias in profile_aliases(profile) {
            let norm = normalize_words(&alias);
            if norm.is_empty() {
                continue;
            }
            if profile.enabled {
                if let Some((other_id, other_name)) = seen_aliases.get(&norm) {
                    if other_id != &profile.id {
                        return Err(format!(
                            "Alias \"{}\" is already used by {}. Remove it there first or rename it here.",
                            alias, other_name
                        ));
                    }
                }
                seen_aliases.insert(norm, (profile.id.clone(), profile.name.clone()));
            }
            let compact_len = alias.chars().filter(|c| c.is_alphanumeric()).count();
            if compact_len > 0 && compact_len < 4 {
                warnings.push(DeveloperProfileWarning {
                    profile_id: profile.id.clone(),
                    alias: Some(alias.clone()),
                    message: "Short aliases may produce more ambiguous matches.".to_string(),
                });
            }
            if is_generic_alias(&alias) {
                warnings.push(DeveloperProfileWarning {
                    profile_id: profile.id.clone(),
                    alias: Some(alias),
                    message: "Generic aliases like \"app\" or \"desktop app\" may be noisy."
                        .to_string(),
                });
            }
        }
    }
    Ok(warnings)
}

fn profile_aliases(profile: &DeveloperProjectProfile) -> Vec<String> {
    let mut aliases = Vec::with_capacity(profile.aliases.len() + 1);
    aliases.push(profile.name.clone());
    aliases.extend(profile.aliases.iter().cloned());
    aliases
}

fn compact_alias_matches(tokens: &[String], alias_norm: &str) -> bool {
    let alias_compact = compact_alnum(alias_norm);
    if alias_compact.len() < 2 {
        return false;
    }
    let max_window = alias_norm.split_whitespace().count().max(2).min(4);
    for start in 0..tokens.len() {
        let mut joined = String::new();
        for token in tokens.iter().skip(start).take(max_window) {
            joined.push_str(&compact_alnum(token));
            if joined == alias_compact {
                return true;
            }
            if joined.len() > alias_compact.len() {
                break;
            }
        }
    }
    false
}

fn normalize_words(value: &str) -> String {
    let mut out = String::new();
    let mut last_space = true;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

fn compact_alnum(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_generic_alias(alias: &str) -> bool {
    matches!(
        normalize_words(alias).as_str(),
        "app" | "desktop app" | "mobile app" | "ios app" | "android app" | "web app"
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn developer_get_settings() -> DeveloperSettingsResponse {
    response_for(load_settings())
}

#[tauri::command]
pub fn developer_save_settings(
    settings: DeveloperSettings,
) -> Result<DeveloperSettingsResponse, String> {
    save_settings(settings)
}

#[tauri::command]
pub fn developer_match_context(transcript: String) -> DeveloperContextMatch {
    let settings = load_settings();
    match_transcript(&transcript, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(profiles: Vec<DeveloperProjectProfile>) -> DeveloperSettings {
        DeveloperSettings {
            enabled: true,
            command_key: "tray".to_string(),
            profiles,
        }
    }

    fn profile(name: &str, aliases: &[&str]) -> DeveloperProjectProfile {
        DeveloperProjectProfile {
            id: name.to_ascii_lowercase().replace(' ', "-"),
            name: name.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            context: "stack: test".to_string(),
            enabled: true,
            source_type: "manual".to_string(),
            updated_at: 1,
        }
    }

    #[test]
    fn matches_short_alias_as_whole_word() {
        let s = settings(vec![profile("HRM8", &["hrm"])]);
        let m = match_transcript("In HRM, find the popup bug.", &s);
        assert_eq!(m.outcome, "project");
        assert_eq!(m.project.unwrap().name, "HRM8");
    }

    #[test]
    fn does_not_match_inside_words() {
        let s = settings(vec![profile("API", &["api"])]);
        let m = match_transcript("This is a capitalized word.", &s);
        assert_eq!(m.outcome, "none");
    }

    #[test]
    fn compact_variant_matches_split_tokens() {
        let s = settings(vec![profile("HRM8", &["hrm8"])]);
        let m = match_transcript("For HRM 8 desktop, check auth.", &s);
        assert_eq!(m.outcome, "project");
    }

    #[test]
    fn ambiguity_hard_stops_when_best_lengths_tie() {
        let s = settings(vec![
            profile("HRM8", &["hrm"]),
            profile("HRM Desktop", &["hrm"]),
        ]);
        let m = match_transcript("In HRM, find the popup bug.", &s);
        assert_eq!(m.outcome, "ambiguous");
    }

    #[test]
    fn duplicate_enabled_aliases_are_blocked_on_save_validation() {
        let s = settings(vec![profile("One", &["api"]), profile("Two", &["api"])]);
        assert!(validate_settings(&s).is_err());
    }
}
