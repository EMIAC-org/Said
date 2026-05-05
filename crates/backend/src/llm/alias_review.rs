use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::store::stt_replacements::{ExportTier, ReviewStatus};

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const REVIEW_MODEL: &str = "llama-3.1-8b-instant";
const OPENAI_FALLBACK_MODEL: &str = "gpt-4.1-nano";

#[derive(Debug, Clone)]
pub struct AliasReviewInput<'a> {
    pub canonical: &'a str,
    pub alias: &'a str,
    pub term_type: Option<&'a str>,
    pub current_tier: ExportTier,
    pub contradiction_count: i64,
    pub use_count: i64,
    pub weight: f64,
    pub example_contexts: &'a [String],
}

#[derive(Debug, Clone)]
pub struct AliasReviewDecision {
    pub export_tier: ExportTier,
    pub review_status: ReviewStatus,
    pub reason: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

#[derive(Deserialize)]
struct ReviewPayload {
    export_tier: String,
    reason: String,
}

const SYSTEM_PROMPT: &str = "You review learned speech-to-text aliases for export to an upstream recognizer. \
You are conservative. Common words and ambiguous aliases should stay local-only or be blocked. \
Only repeated, jargon-like, code-like, acronym-like, or proper-noun aliases should be export_replace_ready. \
Return strict JSON: {\"export_tier\":\"local_only|export_keyterm_support|export_replace_ready|blocked\",\"reason\":\"...\"}.";

pub async fn review_alias(
    client: &Client,
    groq_key: &str,
    openai_key: &str,
    input: &AliasReviewInput<'_>,
) -> Option<AliasReviewDecision> {
    let contexts = if input.example_contexts.is_empty() {
        "- none".to_string()
    } else {
        input
            .example_contexts
            .iter()
            .take(4)
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let user_prompt = format!(
        "CANONICAL: {}\nALIAS: {}\nTERM_TYPE: {}\nCURRENT_TIER: {}\nUSE_COUNT: {}\nWEIGHT: {:.2}\nCONTRADICTIONS: {}\nEXAMPLES:\n{}\n\nReview this alias for upstream export only. Be conservative.",
        input.canonical,
        input.alias,
        input.term_type.unwrap_or("unknown"),
        input.current_tier.as_str(),
        input.use_count,
        input.weight,
        input.contradiction_count,
        contexts
    );

    let groq_result = if !groq_key.is_empty() {
        call_chat(
            client,
            GROQ_ENDPOINT,
            groq_key,
            REVIEW_MODEL,
            &user_prompt,
            "groq",
        )
        .await
    } else {
        None
    };
    let content = if let Some(content) = groq_result {
        Some(content)
    } else if !openai_key.is_empty() {
        call_chat(
            client,
            OPENAI_ENDPOINT,
            openai_key,
            OPENAI_FALLBACK_MODEL,
            &user_prompt,
            "openai",
        )
        .await
    } else {
        None
    }?;

    let payload: ReviewPayload = match serde_json::from_str(content.trim()) {
        Ok(v) => v,
        Err(e) => {
            warn!("[alias-review] parse failed: {e}");
            return None;
        }
    };
    let tier = ExportTier::parse(&payload.export_tier);
    let review_status = if tier == ExportTier::Blocked {
        ReviewStatus::Blocked
    } else {
        ReviewStatus::Approved
    };
    info!(
        "[alias-review] {:?} -> {:?} for alias {:?} -> {:?}",
        input.current_tier, tier, input.alias, input.canonical
    );
    Some(AliasReviewDecision {
        export_tier: tier,
        review_status,
        reason: payload.reason,
    })
}

async fn call_chat(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    user_prompt: &str,
    provider: &str,
) -> Option<String> {
    let body = json!({
        "model": model,
        "temperature": 0.1,
        "max_tokens": 120,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_prompt}
        ]
    });
    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        warn!(
            "[alias-review] {provider} returned {status}: {}",
            &preview[..preview.len().min(200)]
        );
        return None;
    }
    let parsed: ChatResponse = resp.json().await.ok()?;
    parsed.choices.into_iter().next().map(|c| c.message.content)
}
