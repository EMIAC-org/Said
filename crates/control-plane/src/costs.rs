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

#[cfg(test)]
mod tests {
    use super::{gemma_token_cost, together_nemotron_cost};

    #[test]
    fn prices_supplied_rate_card_examples() {
        assert_eq!(together_nemotron_cost(3600.0), Some(0.09));
        assert_eq!(gemma_token_cost(1_000_000, 1_000_000), Some(0.615));
    }
}
