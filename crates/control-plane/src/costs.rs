//! Versioned provider rate cards used only when the provider does not return
//! an authoritative per-request cost. Keep pricing in one place so ingestion,
//! admin reporting, and tests cannot drift.

pub const RATE_EFFECTIVE_FROM: &str = "2026-07-15";
pub const TOGETHER_NEMOTRON_USD_PER_HOUR: f64 = 0.09;
pub const TOGETHER_NEMOTRON_RATE_SOURCE: &str = "rate:together_nemotron_0.09_per_hour@2026-07-15";
pub const GEMMA_INPUT_USD_PER_MILLION: f64 = 0.105;
pub const GEMMA_OUTPUT_USD_PER_MILLION: f64 = 0.51;
pub const GEMMA_RATE_SOURCE: &str =
    "rate:deepinfra_priority_gemma_4_26b_a4b_0.105_in_0.51_out@2026-07-16";

// DeepSeek V4 Flash token rate card. Meeting AI cost is approximated on this
// card (see `ai_worker.rs`), even though the transport is the Codex backend.
pub const DEEPSEEK_V4_FLASH_INPUT_USD_PER_MILLION: f64 = 0.14;
pub const DEEPSEEK_V4_FLASH_CACHE_HIT_USD_PER_MILLION: f64 = 0.0028;
pub const DEEPSEEK_V4_FLASH_OUTPUT_USD_PER_MILLION: f64 = 0.28;
pub const DEEPSEEK_V4_FLASH_RATE_SOURCE: &str =
    "rate:deepseek_v4_flash_0.14_in_0.0028_cache_0.28_out@2026-07-16";

// DeepSeek V4-Pro public USD rate card captured by the control plane. Meeting
// usage stores these values as a per-row JSON snapshot so a later price change
// cannot rewrite historical spend.
pub const DEEPSEEK_V4_PRO_CACHE_HIT_USD_PER_MILLION: f64 = 0.003625;
pub const DEEPSEEK_V4_PRO_CACHE_MISS_USD_PER_MILLION: f64 = 0.435;
pub const DEEPSEEK_V4_PRO_OUTPUT_USD_PER_MILLION: f64 = 0.87;
pub const DEEPSEEK_V4_PRO_RATE_VERSION: &str = "deepseek-v4-pro-usd@2026-07-17";
pub const DEEPSEEK_V4_PRO_RATE_SOURCE: &str =
    "rate:deepseek_v4_pro_0.003625_cache_hit_0.435_cache_miss_0.87_output@2026-07-17";
pub const DEEPSEEK_V4_PRO_PRICING_URL: &str = "https://api-docs.deepseek.com/quick_start/pricing";

pub fn together_nemotron_cost(audio_seconds: f64) -> Option<f64> {
    (audio_seconds.is_finite() && audio_seconds >= 0.0)
        .then_some(audio_seconds * TOGETHER_NEMOTRON_USD_PER_HOUR / 3600.0)
}

pub fn gemma_token_cost(input_tokens: i32, output_tokens: i32) -> Option<f64> {
    (input_tokens >= 0 && output_tokens >= 0).then_some(
        (f64::from(input_tokens) * GEMMA_INPUT_USD_PER_MILLION
            + f64::from(output_tokens) * GEMMA_OUTPUT_USD_PER_MILLION)
            / 1_000_000.0,
    )
}

/// DeepSeek V4 Flash cost. Non-cached input = `(input_tokens - cached_input_tokens).max(0)`
/// priced at the input rate, cached input at the cache-hit rate, output at the output rate.
pub fn deepseek_v4_flash_cost(
    input_tokens: i32,
    cached_input_tokens: i32,
    output_tokens: i32,
) -> Option<f64> {
    if input_tokens < 0 || cached_input_tokens < 0 || output_tokens < 0 {
        return None;
    }
    let non_cached_input = (input_tokens - cached_input_tokens).max(0);
    Some(
        (f64::from(non_cached_input) * DEEPSEEK_V4_FLASH_INPUT_USD_PER_MILLION
            + f64::from(cached_input_tokens) * DEEPSEEK_V4_FLASH_CACHE_HIT_USD_PER_MILLION
            + f64::from(output_tokens) * DEEPSEEK_V4_FLASH_OUTPUT_USD_PER_MILLION)
            / 1_000_000.0,
    )
}

/// DeepSeek V4-Pro cost using the provider's explicit cache-hit/cache-miss
/// counters. `completion_tokens` already includes reasoning tokens, so the
/// optional reasoning detail must not be charged a second time.
pub fn deepseek_v4_pro_cost(
    cache_hit_tokens: i32,
    cache_miss_tokens: i32,
    completion_tokens: i32,
) -> Option<f64> {
    if cache_hit_tokens < 0 || cache_miss_tokens < 0 || completion_tokens < 0 {
        return None;
    }
    Some(
        (f64::from(cache_hit_tokens) * DEEPSEEK_V4_PRO_CACHE_HIT_USD_PER_MILLION
            + f64::from(cache_miss_tokens) * DEEPSEEK_V4_PRO_CACHE_MISS_USD_PER_MILLION
            + f64::from(completion_tokens) * DEEPSEEK_V4_PRO_OUTPUT_USD_PER_MILLION)
            / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        deepseek_v4_flash_cost, deepseek_v4_pro_cost, gemma_token_cost, together_nemotron_cost,
    };

    #[test]
    fn prices_supplied_rate_card_examples() {
        assert_eq!(together_nemotron_cost(3600.0), Some(0.09));
        assert_eq!(gemma_token_cost(1_000_000, 1_000_000), Some(0.615));
    }

    #[test]
    fn prices_deepseek_v4_flash_rate_card_examples() {
        // 1M input (none cached) + 1M output = 0.14 + 0.28 = 0.42.
        assert_eq!(
            deepseek_v4_flash_cost(1_000_000, 0, 1_000_000),
            Some(0.42_f64)
        );
        // Fully cached 1M input = 0.0028; no output.
        assert_eq!(
            deepseek_v4_flash_cost(1_000_000, 1_000_000, 0),
            Some(0.0028)
        );
        assert_eq!(deepseek_v4_flash_cost(-1, 0, 0), None);
    }

    #[test]
    fn prices_deepseek_v4_pro_cache_aware_examples() {
        assert_eq!(deepseek_v4_pro_cost(1_000_000, 0, 0), Some(0.003625));
        assert_eq!(deepseek_v4_pro_cost(0, 1_000_000, 0), Some(0.435));
        assert_eq!(deepseek_v4_pro_cost(0, 0, 1_000_000), Some(0.87));
        assert_eq!(
            deepseek_v4_pro_cost(1_000_000, 1_000_000, 1_000_000),
            Some(1.308625)
        );
        assert_eq!(deepseek_v4_pro_cost(0, -1, 0), None);
    }
}
