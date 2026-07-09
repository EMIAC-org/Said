//! Single source of truth for dictation polish model selection.
//!
//! Catalog keys are stored in `preferences.selected_model`. Each entry maps to
//! a provider module + model id. Beta-only entries are hidden unless beta mode
//! is on in the desktop UI.

/// Fast polish tier — Groq Llama 3.1 8B instant.
pub const GROQ_POLISH_MODEL_FAST: &str = "llama-3.1-8b-instant";

/// Gemma 4 31B on Cerebras — production polish model.
pub const CEREBRAS_POLISH_MODEL_GEMMA_4: &str = "gemma-4-31b";

/// Legacy Groq smart tier id (aliases only; runtime is hard-pinned elsewhere).
pub const GROQ_POLISH_MODEL_SMART_DEFAULT: &str = CEREBRAS_POLISH_MODEL_GEMMA_4;

/// Deprecated GPT OSS alias target. Runtime maps this away from GPT OSS.
pub const GROQ_POLISH_MODEL_BALANCED_DEFAULT: &str = CEREBRAS_POLISH_MODEL_GEMMA_4;

/// Env var — legacy override for Groq 120B id when normalizing old aliases.
pub const SMART_POLISH_MODEL_ENV: &str = "AIRNOTE_SMART_POLISH_MODEL";

/// Production default when beta is off and key is unknown.
pub const DEFAULT_POLISH_MODEL_KEY: &str = "cerebras-gemma-4";

/// One selectable polish model in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolishModelSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub provider: &'static str,
    pub model_id: &'static str,
    /// Reasoning models need `reasoning_effort: low` + higher max_tokens.
    pub reasoning_low: bool,
    /// Shown only when desktop beta mode is enabled.
    pub beta_only: bool,
}

/// Curated polish catalog — plug-and-play with provider modules in `said-backend`.
pub const POLISH_MODEL_CATALOG: &[PolishModelSpec] = &[
    PolishModelSpec {
        key: "cerebras-gemma-4",
        label: "Gemma 4 31B (Cerebras)",
        provider: "cerebras",
        model_id: CEREBRAS_POLISH_MODEL_GEMMA_4,
        reasoning_low: false,
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
        label: "Deprecated GPT OSS alias (Cerebras Gemma 4)",
        provider: "cerebras",
        model_id: GROQ_POLISH_MODEL_BALANCED_DEFAULT,
        reasoning_low: false,
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
        "smart" | "cerebras" | "cerebras-gpt-oss" => Some(DEFAULT_POLISH_MODEL_KEY),
        "fast" | "deepseek" => Some("fast"),
        "scout" | "groq-scout" => Some("groq-scout"),
        "maverick" | "groq-maverick" => Some(DEFAULT_POLISH_MODEL_KEY),
        "phi4" | "phi-4" | "microsoft/phi-4" => Some("phi4"),
        "cerebras-gemma-4" | "gemma-4" | "gemma-4-31b" => Some(DEFAULT_POLISH_MODEL_KEY),
        "groq-gpt-oss-20b" | "gpt-oss-20b" | "openai/gpt-oss-20b" => Some(DEFAULT_POLISH_MODEL_KEY),
        "groq-70b" | "70b" => Some("groq-70b"),
        _ if model.contains("gpt-oss-20b") || model.contains("gpt_oss_20b") => {
            Some(DEFAULT_POLISH_MODEL_KEY)
        }
        _ if model.contains("gpt-oss-120b")
            || model.contains("gpt_oss_120b")
            || model == "gpt-oss-120b" =>
        {
            Some(DEFAULT_POLISH_MODEL_KEY)
        }
        _ if model.contains("gpt-oss") || model.contains("gpt_oss") => {
            Some(DEFAULT_POLISH_MODEL_KEY)
        }
        _ if model.contains("scout") => Some("groq-scout"),
        _ if model.contains("maverick") => Some(DEFAULT_POLISH_MODEL_KEY),
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
///
/// Production polish is hard-pinned to Cerebras Gemma 4. Stored prefs and
/// legacy aliases are still accepted for compatibility, but they cannot change
/// the runtime provider/model.
pub fn resolve_polish_route(_selected_model: &str) -> PolishRoute {
    let key = DEFAULT_POLISH_MODEL_KEY.to_string();
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
            "cerebras-gemma-4"
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
            "cerebras-gemma-4"
        );
        assert_eq!(validate_polish_model_key("smart"), "cerebras-gemma-4");
        assert_eq!(
            validate_polish_model_key("openai/gpt-oss-20b"),
            "cerebras-gemma-4"
        );
        assert_eq!(validate_polish_model_key("llama-3.1-8b-instant"), "fast");
    }

    #[test]
    fn legacy_groq_gpt_oss_20b_routes_cerebras_gemma_4() {
        let route = resolve_polish_route("groq-gpt-oss-20b");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
        assert!(!route.reasoning_low);
    }

    #[test]
    fn legacy_phi4_routes_cerebras_gemma_4() {
        let route = resolve_polish_route("phi4");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
    }

    #[test]
    fn resolve_cerebras_gemma_4() {
        let route = resolve_polish_route("cerebras-gpt-oss");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
        assert!(!route.reasoning_low);
    }

    #[test]
    fn legacy_smart_routes_cerebras_gemma_4() {
        let route = resolve_polish_route("smart");
        assert_eq!(route.key, "cerebras-gemma-4");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
    }

    #[test]
    fn legacy_fast_tier_routes_cerebras_gemma_4() {
        let route = resolve_polish_route("fast");
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
    }

    #[test]
    fn unknown_key_falls_back_to_default() {
        let route = resolve_polish_route("totally-unknown-model");
        assert_eq!(route.key, DEFAULT_POLISH_MODEL_KEY);
        assert_eq!(route.provider, "cerebras");
        assert_eq!(route.model, CEREBRAS_POLISH_MODEL_GEMMA_4);
    }
}
