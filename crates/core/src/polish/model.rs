//! Single source of truth for dictation polish model selection.
//!
//! Keeping this registry narrow prevents the UI, stored preferences, and the
//! runtime from claiming that a different model/provider is in use.

/// Fast Groq model used only by non-polish helpers such as the learning judge.
pub const GROQ_POLISH_MODEL_FAST: &str = "llama-3.1-8b-instant";

/// Smart Groq helper model. This is not the production voice-polish route.
pub const GROQ_POLISH_MODEL_SMART_DEFAULT: &str = "llama-3.3-70b-versatile";

/// Paid Gemma 4 26B A4B via DeepInfra's direct OpenAI-compatible API.
pub const DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B: &str = "google/gemma-4-26B-A4B-it";

/// Fast, low-cost DeepSeek model with thinking disabled at request time.
pub const DEEPSEEK_POLISH_MODEL_V4_FLASH: &str = "deepseek-v4-flash";

/// Default production dictation-polish model.
pub const DEFAULT_POLISH_MODEL_KEY: &str = "deepinfra-gemma-4-26b-a4b";

/// One selectable polish model in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolishModelSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub provider: &'static str,
    pub model_id: &'static str,
    /// Reasoning models need special request settings.
    pub reasoning_low: bool,
    /// Shown only when desktop beta mode is enabled.
    pub beta_only: bool,
}

/// Curated production catalog shared by preferences and runtime routing.
pub const POLISH_MODEL_CATALOG: &[PolishModelSpec] = &[
    PolishModelSpec {
        key: DEFAULT_POLISH_MODEL_KEY,
        label: "Gemma 4 26B A4B (DeepInfra)",
        provider: "deepinfra",
        model_id: DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B,
        reasoning_low: false,
        beta_only: false,
    },
    PolishModelSpec {
        key: DEEPSEEK_POLISH_MODEL_V4_FLASH,
        label: "DeepSeek V4 Flash (No reasoning)",
        provider: "deepseek",
        model_id: DEEPSEEK_POLISH_MODEL_V4_FLASH,
        reasoning_low: false,
        beta_only: false,
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

/// Legacy helper used by the learning judge, which still calls Groq directly.
pub fn groq_polish_model_smart() -> String {
    GROQ_POLISH_MODEL_SMART_DEFAULT.to_string()
}

pub fn catalog_spec(key: &str) -> Option<&'static PolishModelSpec> {
    let key = key.trim().to_ascii_lowercase();
    POLISH_MODEL_CATALOG.iter().find(|spec| spec.key == key)
}

/// Old selections resolve to the default production route. We deliberately do
/// not retain provider-specific aliases: deployments migrate stored values.
pub fn validate_polish_model_key(raw: &str) -> String {
    catalog_spec(raw)
        .map(|spec| spec.key.to_string())
        .unwrap_or_else(|| DEFAULT_POLISH_MODEL_KEY.to_string())
}

pub fn normalize_selected_model(raw: &str) -> String {
    validate_polish_model_key(raw)
}

pub fn resolve_polish_route(selected_model: &str) -> PolishRoute {
    let key = validate_polish_model_key(selected_model);
    let spec = catalog_spec(&key).expect("validated polish catalog entry");
    PolishRoute {
        key: spec.key.to_string(),
        provider: spec.provider,
        model: spec.model_id.to_string(),
        reasoning_low: spec.reasoning_low,
    }
}

pub fn polish_model_display_label(selected_model: &str) -> String {
    let key = validate_polish_model_key(selected_model);
    catalog_spec(&key)
        .map(|spec| spec.label.to_string())
        .unwrap_or(key)
}

/// Compatibility helper for call sites that only need the model identifier.
pub fn resolve_groq_polish_model(selected_model: &str) -> String {
    resolve_polish_route(selected_model).model
}

pub fn polish_model_label(selected_model: &str) -> String {
    resolve_polish_route(selected_model).label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_selection_normalizes_to_gemma() {
        assert_eq!(validate_polish_model_key("smart"), DEFAULT_POLISH_MODEL_KEY);
        assert_eq!(
            validate_polish_model_key("anything-old"),
            DEFAULT_POLISH_MODEL_KEY
        );
    }

    #[test]
    fn default_route_is_paid_deepinfra_gemma() {
        let route = resolve_polish_route("anything");
        assert_eq!(route.provider, "deepinfra");
        assert_eq!(route.model, DEEPINFRA_POLISH_MODEL_GEMMA_4_26B_A4B);
        assert!(!route.reasoning_low);
    }

    #[test]
    fn deepseek_v4_flash_routes_to_deepseek() {
        let route = resolve_polish_route(DEEPSEEK_POLISH_MODEL_V4_FLASH);
        assert_eq!(route.key, DEEPSEEK_POLISH_MODEL_V4_FLASH);
        assert_eq!(route.provider, "deepseek");
        assert_eq!(route.model, DEEPSEEK_POLISH_MODEL_V4_FLASH);
        assert!(!route.reasoning_low);
    }
}
