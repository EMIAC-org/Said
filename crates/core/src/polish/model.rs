//! Single source of truth for dictation polish model selection.
//!
//! Catalog keys are stored in `preferences.selected_model`. Each entry maps to
//! a provider module + model id. Beta-only entries are hidden unless beta mode
//! is on in the desktop UI.

/// Fast polish tier — Groq Llama 3.1 8B instant.
pub const GROQ_POLISH_MODEL_FAST: &str = "llama-3.1-8b-instant";

/// GPT OSS 120B on Cerebras — production default polish model.
pub const CEREBRAS_POLISH_MODEL_GPT_OSS: &str = "gpt-oss-120b";

/// Legacy Groq smart tier id (aliases only; routing uses Cerebras).
pub const GROQ_POLISH_MODEL_SMART_DEFAULT: &str = "openai/gpt-oss-120b";

/// Groq balanced tier — GPT OSS 20B (beta only).
pub const GROQ_POLISH_MODEL_BALANCED_DEFAULT: &str = "openai/gpt-oss-20b";

/// Env var — legacy override for Groq 120B id when normalizing old aliases.
pub const SMART_POLISH_MODEL_ENV: &str = "AIRNOTE_SMART_POLISH_MODEL";

/// Production default when beta is off and key is unknown.
pub const DEFAULT_POLISH_MODEL_KEY: &str = "cerebras-gpt-oss";

/// One selectable polish model in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolishModelSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub provider: &'static str,
    pub model_id: &'static str,
    /// GPT-OSS style models need `reasoning_effort: low` + higher max_tokens.
    pub reasoning_low: bool,
    /// Shown only when desktop beta mode is enabled.
    pub beta_only: bool,
}

/// Curated polish catalog — plug-and-play with provider modules in `said-backend`.
pub const POLISH_MODEL_CATALOG: &[PolishModelSpec] = &[
    PolishModelSpec {
        key: "cerebras-gpt-oss",
        label: "GPT OSS 120B (Cerebras)",
        provider: "cerebras",
        model_id: CEREBRAS_POLISH_MODEL_GPT_OSS,
        reasoning_low: true,
        beta_only: false,
    },
    PolishModelSpec {
        key: "fast",
        label: "8B Instant (Groq)",
        provider: "groq",
        model_id: GROQ_POLISH_MODEL_FAST,
        reasoning_low: false,
        beta_only: false,
    },
    PolishModelSpec {
        key: "groq-gpt-oss-20b",
        label: "GPT OSS 20B (Groq)",
        provider: "groq",
        model_id: GROQ_POLISH_MODEL_BALANCED_DEFAULT,
        reasoning_low: true,
        beta_only: true,
    },
    PolishModelSpec {
        key: "groq-scout",
        label: "Scout 17B (Groq)",
        provider: "groq",
        model_id: "meta-llama/llama-4-scout-17b-16e-instruct",
        reasoning_low: false,
        beta_only: true,
    },
    PolishModelSpec {
        key: "groq-70b",
        label: "Llama 3.3 70B (Groq)",
        provider: "groq",
        model_id: "llama-3.3-70b-versatile",
        reasoning_low: false,
        beta_only: true,
    },
    PolishModelSpec {
        key: "phi4",
        label: "Phi-4 (DeepInfra)",
        provider: "deepinfra",
        model_id: "microsoft/phi-4",
        reasoning_low: false,
        beta_only: true,
    },
];

/// Where a polish request should be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolishRoute {
    pub key: String,
    pub provider: &'static str,
    pub model: String,
    pub reasoning_low: bool,
}

impl PolishRoute {
    pub fn label(&self) -> String {
        format!("{}:{}", self.provider, self.model)
    }
}

/// Legacy Groq 120B model id resolver (alias normalization only).
pub fn groq_polish_model_smart() -> String {
    std::env::var(SMART_POLISH_MODEL_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| GROQ_POLISH_MODEL_SMART_DEFAULT.to_string())
}

pub fn catalog_spec(key: &str) -> Option<&'static PolishModelSpec> {
    let key = key.trim().to_ascii_lowercase();
    POLISH_MODEL_CATALOG.iter().find(|spec| spec.key == key)
}

/// Legacy alias → canonical catalog key (for migration reads / old clients).
pub fn legacy_alias_to_key(raw: &str) -> Option<&'static str> {
    let model = raw.trim().to_ascii_lowercase();
    if catalog_spec(&model).is_some() {
        return Some(
            POLISH_MODEL_CATALOG
                .iter()
                .find(|s| s.key == model)
                .map(|s| s.key)
                .unwrap_or(DEFAULT_POLISH_MODEL_KEY),
        );
    }
    match model.as_str() {
        "smart" | "cerebras" => Some("cerebras-gpt-oss"),
        "fast" | "deepseek" => Some("fast"),
        "scout" | "groq-scout" => Some("groq-scout"),
        "maverick" | "groq-maverick" => Some("cerebras-gpt-oss"),
        "phi4" | "phi-4" | "microsoft/phi-4" => Some("phi4"),
        "cerebras-gpt-oss" => Some("cerebras-gpt-oss"),
        "groq-gpt-oss-20b" | "gpt-oss-20b" | "openai/gpt-oss-20b" => Some("groq-gpt-oss-20b"),
        "groq-70b" | "70b" => Some("groq-70b"),
        _ if model.contains("gpt-oss-20b") || model.contains("gpt_oss_20b") => {
            Some("groq-gpt-oss-20b")
        }
        _ if model.contains("gpt-oss-120b")
            || model.contains("gpt_oss_120b")
            || model == "gpt-oss-120b" =>
        {
            Some("cerebras-gpt-oss")
        }
        _ if model.contains("gpt-oss") || model.contains("gpt_oss") => Some("cerebras-gpt-oss"),
        _ if model.contains("scout") => Some("groq-scout"),
        _ if model.contains("maverick") => Some("cerebras-gpt-oss"),
        _ if model.contains("70b") && model.contains("versatile") => Some("groq-70b"),
        _ if model.contains("8b") || model.contains("instant") => Some("fast"),
        _ => None,
    }
}

/// Canonical catalog key for persisting in SQLite (preserves beta keys).
pub fn validate_polish_model_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_POLISH_MODEL_KEY.to_string();
    }
    if let Some(spec) = catalog_spec(trimmed) {
        return spec.key.to_string();
    }
    if let Some(key) = legacy_alias_to_key(trimmed) {
        return key.to_string();
    }
    DEFAULT_POLISH_MODEL_KEY.to_string()
}

/// Collapse legacy aliases to canonical catalog keys (preserves beta keys).
pub fn normalize_selected_model(raw: &str) -> String {
    validate_polish_model_key(raw)
}

fn resolve_model_id(spec: &PolishModelSpec) -> String {
    spec.model_id.to_string()
}

/// Primary entry point — picks provider + model for any polish request.
pub fn resolve_polish_route(selected_model: &str) -> PolishRoute {
    let key = validate_polish_model_key(selected_model);
    let spec = catalog_spec(&key).unwrap_or_else(|| {
        POLISH_MODEL_CATALOG
            .iter()
            .find(|s| s.key == DEFAULT_POLISH_MODEL_KEY)
            .expect("default polish catalog entry")
    });
    PolishRoute {
        key: spec.key.to_string(),
        provider: spec.provider,
        model: resolve_model_id(spec),
        reasoning_low: spec.reasoning_low,
    }
}

/// List catalog entries for UI (`beta` includes experimental models).
pub fn list_polish_models(beta: bool) -> Vec<PolishModelSpec> {
    POLISH_MODEL_CATALOG
        .iter()
        .copied()
        .filter(|spec| !spec.beta_only || beta)
        .collect()
}

/// Human-readable label for a stored catalog key.
pub fn polish_model_display_label(selected_model: &str) -> String {
    let key = validate_polish_model_key(selected_model);
    catalog_spec(&key)
        .map(|s| s.label.to_string())
        .unwrap_or_else(|| key)
}

/// Map a prefs `selected_model` value to the Groq model id (legacy helper).
pub fn resolve_groq_polish_model(selected_model: &str) -> String {
    resolve_polish_route(selected_model).model
}

/// Human/log label — provider + model that will run.
pub fn polish_model_label(selected_model: &str) -> String {
    resolve_polish_route(selected_model).label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_preserves_beta_keys() {
        assert_eq!(validate_polish_model_key("phi4"), "phi4");
        assert_eq!(validate_polish_model_key("groq-scout"), "groq-scout");
        assert_eq!(
            validate_polish_model_key("cerebras-gpt-oss"),
            "cerebras-gpt-oss"
        );
        assert_eq!(
            validate_polish_model_key("groq-gpt-oss-20b"),
            "groq-gpt-oss-20b"
        );
    }

    #[test]
    fn legacy_aliases_map_to_catalog_keys() {
        assert_eq!(validate_polish_model_key("scout"), "groq-scout");
        assert_eq!(
            validate_polish_model_key("openai/gpt-oss-120b"),
            "cerebras-gpt-oss"
        );
        assert_eq!(validate_polish_model_key("smart"), "cerebras-gpt-oss");
        assert_eq!(
            validate_polish_model_key("openai/gpt-oss-20b"),
            "groq-gpt-oss-20b"
        );
        assert_eq!(validate_polish_model_key("llama-3.1-8b-instant"), "fast");
    }

    #[test]
    fn resolve_groq_gpt_oss_20b() {
        let route = resolve_polish_route("groq-gpt-oss-20b");
        assert_eq!(route.provider, "groq");
        assert_eq!(route.model, "openai/gpt-oss-20b");
        assert!(route.reasoning_low);
    }

    #[test]
    fn resolve_phi4_routes_deepinfra() {
        let route = resolve_polish_route("phi4");
        assert_eq!(route.provider, "deepinfra");
        assert_eq!(route.model, "microsoft/phi-4");
    }

    #[test]
    fn resolve_cerebras_gpt_oss() {
        let route = resolve_polish_route("cerebras-gpt-oss");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GPT_OSS);
        assert!(route.reasoning_low);
    }

    #[test]
    fn legacy_smart_routes_cerebras_gpt_oss() {
        let route = resolve_polish_route("smart");
        assert_eq!(route.key, "cerebras-gpt-oss");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GPT_OSS);
    }

    #[test]
    fn resolve_fast_tier_stays_on_groq() {
        let route = resolve_polish_route("fast");
        assert_eq!(route.provider, "groq");
        assert_eq!(route.model, GROQ_POLISH_MODEL_FAST);
    }

    #[test]
    fn list_models_beta_filters() {
        let prod = list_polish_models(false);
        assert!(prod.iter().any(|s| s.key == "cerebras-gpt-oss"));
        assert!(prod.iter().any(|s| s.key == "fast"));
        assert!(!prod.iter().any(|s| s.key == "groq-gpt-oss-20b"));
        let beta = list_polish_models(true);
        assert!(beta.iter().any(|s| s.key == "groq-gpt-oss-20b"));
        assert!(beta.iter().any(|s| s.key == "phi4"));
    }

    #[test]
    fn unknown_key_falls_back_to_default() {
        let route = resolve_polish_route("totally-unknown-model");
        assert_eq!(route.key, DEFAULT_POLISH_MODEL_KEY);
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GPT_OSS);
    }
}
