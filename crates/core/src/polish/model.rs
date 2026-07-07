//! Single source of truth for dictation polish model selection.
//!
//! Catalog keys are stored in `preferences.selected_model`. Each entry maps to
//! a provider module + model id. Beta-only entries are hidden unless beta mode
//! is on in the desktop UI.

/// Fast polish tier — Groq Llama 3.1 8B instant.
pub const GROQ_POLISH_MODEL_FAST: &str = "llama-3.1-8b-instant";

/// GPT OSS 120B on Cerebras — legacy/fallback polish model.
pub const CEREBRAS_POLISH_MODEL_GPT_OSS: &str = "gpt-oss-120b";

/// Gemma 4 31B via OpenRouter — production default polish model (benchmark
/// winner: highest garble-correction recall). Served through the OpenRouter
/// OpenAI-compatible endpoint.
pub const OPENROUTER_POLISH_MODEL_GEMMA: &str = "google/gemma-4-31b-it";

/// OpenRouter sub-provider routing order for the Gemma polish model. Default
/// routing picks slow hosts (DeepInfra/Parasail measured at 15-20s); these two
/// serve it fast (WandB ~0.7s, ModelRun ~1.1s). The dispatch pins this order
/// with fallbacks enabled, so a slow host is only used if both fast ones are down.
pub const OPENROUTER_GEMMA_PROVIDERS: &[&str] = &["WandB", "ModelRun"];

// ── Provider endpoints (all OpenAI-compatible /chat/completions) ─────────────
// Adapter model: a catalog entry carries its own endpoint + key env var +
// (optional) sub-provider order + reasoning flag. Switching or adding a model is
// ONE catalog row — no dispatch/AppState/credential-map edits. The reasoning
// SHAPE is inferred from the endpoint (OpenRouter uses a `reasoning` object;
// everyone else a top-level `reasoning_effort`).
pub const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
pub const CEREBRAS_ENDPOINT: &str = "https://api.cerebras.ai/v1/chat/completions";
pub const DEEPINFRA_ENDPOINT: &str = "https://api.deepinfra.com/v1/openai/chat/completions";
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Env var that force-overrides the polish model catalog key for EVERY request,
/// regardless of the per-account stored `selected_model`. The one-line switch:
/// set `POLISH_MODEL_OVERRIDE=gemma-openrouter` on the server and everything
/// routes there. Empty/unset = honor the per-account selection.
pub const POLISH_MODEL_OVERRIDE_ENV: &str = "POLISH_MODEL_OVERRIDE";

/// Legacy Groq smart tier id (aliases only; routing uses Cerebras).
pub const GROQ_POLISH_MODEL_SMART_DEFAULT: &str = "openai/gpt-oss-120b";

/// Groq balanced tier — GPT OSS 20B (beta only).
pub const GROQ_POLISH_MODEL_BALANCED_DEFAULT: &str = "openai/gpt-oss-20b";

/// Env var — legacy override for Groq 120B id when normalizing old aliases.
pub const SMART_POLISH_MODEL_ENV: &str = "AIRNOTE_SMART_POLISH_MODEL";

/// Production default when beta is off and key is unknown.
pub const DEFAULT_POLISH_MODEL_KEY: &str = "gemma-openrouter";

/// One selectable polish model in the catalog. Everything the dispatch needs to
/// place the call lives here — so adding/switching a model is one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolishModelSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// Coarse provider tag (kept for logging + the two non-OpenAI-compatible
    /// legacy modules). New OpenAI-compatible models don't need a new tag.
    pub provider: &'static str,
    pub model_id: &'static str,
    /// OpenAI-compatible chat/completions URL this model is called on.
    pub endpoint: &'static str,
    /// Env var holding the API key for `endpoint` (e.g. "OPENROUTER_API_KEY").
    pub api_key_env: &'static str,
    /// OpenRouter sub-provider routing order (empty for non-OpenRouter models).
    pub providers: &'static [&'static str],
    /// Send `reasoning_effort: low` (shape inferred from the endpoint).
    pub reasoning_low: bool,
    /// Shown only when desktop beta mode is enabled.
    pub beta_only: bool,
}

/// Curated polish catalog. To add a model: copy a row, set key/label/model_id/
/// endpoint/api_key_env (+ providers for OpenRouter, + reasoning_low for
/// thinking models). Nothing else in the codebase needs to change.
pub const POLISH_MODEL_CATALOG: &[PolishModelSpec] = &[
    PolishModelSpec {
        key: "gemma-openrouter",
        label: "Gemma 4 31B (OpenRouter)",
        provider: "openrouter",
        model_id: OPENROUTER_POLISH_MODEL_GEMMA,
        endpoint: OPENROUTER_ENDPOINT,
        api_key_env: "OPENROUTER_API_KEY",
        providers: OPENROUTER_GEMMA_PROVIDERS,
        reasoning_low: false,
        beta_only: false,
    },
    PolishModelSpec {
        key: "cerebras-gpt-oss",
        label: "GPT OSS 120B (Cerebras)",
        provider: "cerebras",
        model_id: CEREBRAS_POLISH_MODEL_GPT_OSS,
        endpoint: CEREBRAS_ENDPOINT,
        api_key_env: "CEREBRAS_API_KEY",
        providers: &[],
        reasoning_low: true,
        beta_only: false,
    },
    PolishModelSpec {
        key: "fast",
        label: "8B Instant (Groq)",
        provider: "groq",
        model_id: GROQ_POLISH_MODEL_FAST,
        endpoint: GROQ_ENDPOINT,
        api_key_env: "GROQ_API_KEY",
        providers: &[],
        reasoning_low: false,
        beta_only: false,
    },
    PolishModelSpec {
        key: "groq-gpt-oss-20b",
        label: "GPT OSS 20B (Groq)",
        provider: "groq",
        model_id: GROQ_POLISH_MODEL_BALANCED_DEFAULT,
        endpoint: GROQ_ENDPOINT,
        api_key_env: "GROQ_API_KEY",
        providers: &[],
        reasoning_low: true,
        beta_only: true,
    },
    PolishModelSpec {
        key: "groq-scout",
        label: "Scout 17B (Groq)",
        provider: "groq",
        model_id: "meta-llama/llama-4-scout-17b-16e-instruct",
        endpoint: GROQ_ENDPOINT,
        api_key_env: "GROQ_API_KEY",
        providers: &[],
        reasoning_low: false,
        beta_only: true,
    },
    PolishModelSpec {
        key: "groq-70b",
        label: "Llama 3.3 70B (Groq)",
        provider: "groq",
        model_id: "llama-3.3-70b-versatile",
        endpoint: GROQ_ENDPOINT,
        api_key_env: "GROQ_API_KEY",
        providers: &[],
        reasoning_low: false,
        beta_only: true,
    },
    PolishModelSpec {
        key: "phi4",
        label: "Phi-4 (DeepInfra)",
        provider: "deepinfra",
        model_id: "microsoft/phi-4",
        endpoint: DEEPINFRA_ENDPOINT,
        api_key_env: "DEEPINFRA_API_KEY",
        providers: &[],
        reasoning_low: false,
        beta_only: true,
    },
];

/// Where a polish request should be sent. Fully self-describing — the caller
/// needs nothing beyond this to place the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolishRoute {
    pub key: String,
    pub provider: &'static str,
    pub model: String,
    pub endpoint: &'static str,
    pub api_key_env: &'static str,
    pub providers: &'static [&'static str],
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
///
/// `POLISH_MODEL_OVERRIDE` (env), when set to a catalog key, wins over the
/// per-account `selected_model` for EVERY request — the one-line global switch
/// for rolling out or A/B-ing a model without touching stored prefs.
pub fn resolve_polish_route(selected_model: &str) -> PolishRoute {
    let key = std::env::var(POLISH_MODEL_OVERRIDE_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && catalog_spec(v).is_some())
        .unwrap_or_else(|| validate_polish_model_key(selected_model));
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
        endpoint: spec.endpoint,
        api_key_env: spec.api_key_env,
        providers: spec.providers,
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
        assert_eq!(route.provider, "openrouter");
        assert_eq!(route.model, OPENROUTER_POLISH_MODEL_GEMMA);
    }

    #[test]
    fn resolve_gemma_openrouter_is_default() {
        let route = resolve_polish_route("gemma-openrouter");
        assert_eq!(route.provider, "openrouter");
        assert_eq!(route.model, OPENROUTER_POLISH_MODEL_GEMMA);
        assert!(!route.reasoning_low);
        assert_eq!(DEFAULT_POLISH_MODEL_KEY, "gemma-openrouter");
    }

    #[test]
    fn route_is_self_describing_adapter() {
        // Every catalog row must carry a usable endpoint + key env; OpenRouter
        // rows may pin sub-providers, others must not.
        for spec in POLISH_MODEL_CATALOG {
            assert!(
                spec.endpoint.starts_with("https://"),
                "{} missing endpoint",
                spec.key
            );
            assert!(
                spec.api_key_env.ends_with("_API_KEY"),
                "{} bad key env",
                spec.key
            );
            if spec.provider != "openrouter" {
                assert!(spec.providers.is_empty(), "{} pins providers", spec.key);
            }
        }
        let g = resolve_polish_route("gemma-openrouter");
        assert_eq!(g.endpoint, OPENROUTER_ENDPOINT);
        assert_eq!(g.api_key_env, "OPENROUTER_API_KEY");
        assert_eq!(g.providers, OPENROUTER_GEMMA_PROVIDERS);
        let c = resolve_polish_route("cerebras-gpt-oss");
        assert_eq!(c.endpoint, CEREBRAS_ENDPOINT);
        assert_eq!(c.api_key_env, "CEREBRAS_API_KEY");
        assert!(c.providers.is_empty());
    }

    #[test]
    fn override_env_forces_model_over_stored_pref() {
        // SAFETY: this test owns the env var for its body; no other test reads it.
        unsafe { std::env::set_var(POLISH_MODEL_OVERRIDE_ENV, "cerebras-gpt-oss") };
        let route = resolve_polish_route("gemma-openrouter"); // stored pref says gemma
        assert_eq!(route.key, "cerebras-gpt-oss", "override must win");
        unsafe { std::env::remove_var(POLISH_MODEL_OVERRIDE_ENV) };
        let route = resolve_polish_route("gemma-openrouter");
        assert_eq!(route.key, "gemma-openrouter", "no override honors pref");
    }
}
