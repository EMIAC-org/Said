//! DeepSeek API client for profile learn-from-edit.

use serde_json::Value;
use tracing::{info, warn};

use crate::AppState;
use crate::profile::updater::prompt::{
    PROFILE_ALIAS_EXPANSION_SYSTEM_PROMPT, PROFILE_BATCH_SYSTEM_PROMPT,
    PROFILE_UPDATE_SYSTEM_PROMPT,
};
use crate::profile::updater::types::{
    BatchProfileResponse, DeepSeekAliasProposal, DeepSeekProfileUpdateResponse,
    ProfileUpdateRequest,
};

const REQUEST_TIMEOUT_SECS: u64 = 20;
const MAX_RETRIES: u32 = 2;

pub fn profile_update_model() -> String {
    std::env::var("DEEPSEEK_PROFILE_UPDATE_MODEL")
        .unwrap_or_else(|_| "deepseek-v4-flash".to_string())
}

pub async fn call_deepseek_profile_update(
    state: &AppState,
    request: &ProfileUpdateRequest,
) -> Result<(DeepSeekProfileUpdateResponse, u64), String> {
    if state.deepseek_api_key.trim().is_empty() {
        return Err("DEEPSEEK_API_KEY is not configured".to_string());
    }

    let user_message = serde_json::to_string_pretty(request)
        .map_err(|e| format!("request serialize failed: {e}"))?;
    let model = profile_update_model();
    let url = format!(
        "{}/v1/chat/completions",
        state.deepseek_base_url.trim_end_matches('/')
    );

    let body = serde_json::json!({
        "model": model,
        "temperature": 0.1,
        "top_p": 0.9,
        // Reasoning ("thinking") is enabled; keep the ceiling high so a reasoning
        // preamble can never truncate the JSON answer. Billed on tokens actually
        // generated, so the cap is purely a truncation safeguard.
        "max_tokens": 8192,
        "stream": false,
        "response_format": { "type": "json_object" },
        // High-effort reasoning. `thinking.enabled` turns the chain-of-thought on;
        // `reasoning_effort` grades it — both are sent because a bare `thinking`
        // toggle is treated as un-graded by some gateways (reasoning goes to the
        // separate `reasoning_content` field, so the JSON `content` parse is safe).
        "thinking": { "type": "enabled" },
        "reasoning_effort": "high",
        "messages": [
            { "role": "system", "content": PROFILE_UPDATE_SYSTEM_PROMPT },
            { "role": "user", "content": user_message }
        ]
    });

    let mut last_err = String::from("unknown DeepSeek error");
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff_ms = if attempt == 1 { 1000 } else { 3000 };
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        }

        let started = std::time::Instant::now();
        info!(
            "[profile-updater] deepseek update start request_id={} account={} org_scope={} edit_event={} attempt={} model={} ai_chars={} kept_chars={} raw_chars={} edit_spans={} current_version={}",
            request.request_id,
            request.account_id,
            request.org_scope,
            request.edit.edit_event_id,
            attempt + 1,
            model,
            request.edit.ai_output.chars().count(),
            request.edit.user_kept.chars().count(),
            request
                .edit
                .raw_transcript
                .as_deref()
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or(0),
            request.edit.edit_spans.len(),
            request.current_profile.version,
        );
        let resp = crate::HTTP_CLIENT
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", state.deepseek_api_key),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let value: Value = r
                    .json()
                    .await
                    .map_err(|e| format!("DeepSeek response parse failed: {e}"))?;
                let raw = value
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if raw.is_empty() {
                    last_err = "DeepSeek returned empty output".to_string();
                    continue;
                }
                let parsed = parse_profile_update_response(&raw)?;
                info!(
                    "[profile-updater] deepseek update complete request_id={} edit_event={} attempt={} latency_ms={} raw_chars={} class={:?} confidence={:.2} background={} focus_areas={} speech_patterns={} recent_context={} terms={} aliases={} markdown_mode={} review_required={} reason=\"{}\"",
                    request.request_id,
                    request.edit.edit_event_id,
                    attempt + 1,
                    started.elapsed().as_millis(),
                    raw.chars().count(),
                    parsed.classification,
                    parsed.confidence,
                    parsed.profile_patch.user_background.is_some(),
                    parsed.profile_patch.add_focus_areas.len(),
                    parsed.profile_patch.add_speech_patterns.len(),
                    parsed.profile_patch.add_recent_context.len(),
                    parsed.profile_patch.add_stable_terms.len(),
                    parsed.alias_proposals.len(),
                    parsed
                        .profile_markdown_patch
                        .mode
                        .as_deref()
                        .unwrap_or("null"),
                    parsed.review_required,
                    said_core::text::truncate_utf8(&parsed.reason, 180),
                );
                return Ok((parsed, started.elapsed().as_millis() as u64));
            }
            Ok(r) => {
                let status = r.status();
                let preview = r.text().await.unwrap_or_default();
                last_err = format!(
                    "DeepSeek HTTP {status}: {}",
                    preview.chars().take(200).collect::<String>()
                );
                warn!("[profile-updater] {last_err}");
            }
            Err(e) => {
                last_err = format!("DeepSeek request failed: {e}");
                warn!("[profile-updater] {last_err}");
            }
        }
    }

    Err(last_err)
}

pub async fn call_deepseek_alias_expansion(
    state: &AppState,
    request: &Value,
) -> Result<(Vec<DeepSeekAliasProposal>, String, u64), String> {
    if state.deepseek_api_key.trim().is_empty() {
        return Err("DEEPSEEK_API_KEY is not configured".to_string());
    }

    let user_message = serde_json::to_string_pretty(request)
        .map_err(|e| format!("alias request serialize failed: {e}"))?;
    let model = profile_update_model();
    let url = format!(
        "{}/v1/chat/completions",
        state.deepseek_base_url.trim_end_matches('/')
    );

    let body = serde_json::json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.8,
        // Reasoning enabled — ceiling raised from the old output-only cap so the
        // JSON answer survives a reasoning preamble (billed on tokens generated).
        "max_tokens": 4096,
        "stream": false,
        "response_format": { "type": "json_object" },
        // High-effort reasoning. `thinking.enabled` turns the chain-of-thought on;
        // `reasoning_effort` grades it — both are sent because a bare `thinking`
        // toggle is treated as un-graded by some gateways (reasoning goes to the
        // separate `reasoning_content` field, so the JSON `content` parse is safe).
        "thinking": { "type": "enabled" },
        "reasoning_effort": "high",
        "messages": [
            { "role": "system", "content": PROFILE_ALIAS_EXPANSION_SYSTEM_PROMPT },
            { "role": "user", "content": user_message }
        ]
    });

    let started = std::time::Instant::now();
    info!(
        "[profile-updater] deepseek alias-expansion start job={} edit_event={} model={} approved_terms={} existing_aliases={}",
        request
            .get("job_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        request
            .get("edit_event_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        model,
        request
            .get("approved_profile_patch")
            .and_then(|v| v.get("add_stable_terms"))
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        request
            .get("current_aliases_after_proposal")
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
    );
    let resp = crate::HTTP_CLIENT
        .post(&url)
        .header(
            "Authorization",
            format!("Bearer {}", state.deepseek_api_key),
        )
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("DeepSeek alias request failed: {e}"))?;

    let status = resp.status();
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("DeepSeek alias response parse failed: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "DeepSeek alias HTTP {status}: {}",
            value.to_string().chars().take(200).collect::<String>()
        ));
    }
    let raw = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if raw.is_empty() {
        return Err("DeepSeek alias expansion returned empty output".to_string());
    }
    let parsed = parse_alias_expansion_response(&raw)?;
    info!(
        "[profile-updater] deepseek alias-expansion complete job={} edit_event={} latency_ms={} raw_chars={} proposed_aliases={} reason=\"{}\"",
        request
            .get("job_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        request
            .get("edit_event_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        started.elapsed().as_millis(),
        raw.chars().count(),
        parsed.0.len(),
        said_core::text::truncate_utf8(&parsed.1, 180),
    );
    Ok((parsed.0, parsed.1, started.elapsed().as_millis() as u64))
}

/// Batched per-bucket profiling + KB call over a window of dictations. `request` is a
/// JSON object built by the batch worker (bucket, runs, current style, unknown apps).
pub async fn call_deepseek_batch_profile(
    state: &AppState,
    request: &Value,
) -> Result<(BatchProfileResponse, u64), String> {
    if state.deepseek_api_key.trim().is_empty() {
        return Err("DEEPSEEK_API_KEY is not configured".to_string());
    }
    let user_message = serde_json::to_string_pretty(request)
        .map_err(|e| format!("batch request serialize failed: {e}"))?;
    let model = profile_update_model();
    let url = format!(
        "{}/v1/chat/completions",
        state.deepseek_base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": model,
        "temperature": 0.1,
        "top_p": 0.9,
        // Reasoning enabled — per-bucket overlays are the largest output of the
        // three calls; keep headroom so reasoning + JSON never collide with the cap.
        "max_tokens": 6144,
        "stream": false,
        "response_format": { "type": "json_object" },
        // High-effort reasoning. `thinking.enabled` turns the chain-of-thought on;
        // `reasoning_effort` grades it — both are sent because a bare `thinking`
        // toggle is treated as un-graded by some gateways (reasoning goes to the
        // separate `reasoning_content` field, so the JSON `content` parse is safe).
        "thinking": { "type": "enabled" },
        "reasoning_effort": "high",
        "messages": [
            { "role": "system", "content": PROFILE_BATCH_SYSTEM_PROMPT },
            { "role": "user", "content": user_message }
        ]
    });

    let mut last_err = String::from("unknown DeepSeek error");
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff_ms = if attempt == 1 { 1000 } else { 3000 };
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        }
        let started = std::time::Instant::now();
        let resp = crate::HTTP_CLIENT
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", state.deepseek_api_key),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let value: Value = r
                    .json()
                    .await
                    .map_err(|e| format!("batch response parse failed: {e}"))?;
                let raw = value
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if raw.is_empty() {
                    last_err = "DeepSeek returned empty batch output".to_string();
                    continue;
                }
                let parsed = parse_batch_profile_response(&raw)?;
                info!(
                    "[profile-batch] deepseek batch complete attempt={} latency_ms={} apply={} confidence={:.2} style_updates={} domains={} app_suggestions={}",
                    attempt + 1,
                    started.elapsed().as_millis(),
                    parsed.apply,
                    parsed.confidence,
                    parsed.style_updates.len(),
                    parsed.add_domains.len(),
                    parsed.app_bucket_suggestions.len(),
                );
                return Ok((parsed, started.elapsed().as_millis() as u64));
            }
            Ok(r) => {
                let status = r.status();
                let preview = r.text().await.unwrap_or_default();
                last_err = format!(
                    "DeepSeek batch HTTP {status}: {}",
                    preview.chars().take(200).collect::<String>()
                );
                warn!("[profile-batch] {last_err}");
            }
            Err(e) => {
                last_err = format!("DeepSeek batch request failed: {e}");
                warn!("[profile-batch] {last_err}");
            }
        }
    }
    Err(last_err)
}

pub fn parse_batch_profile_response(raw: &str) -> Result<BatchProfileResponse, String> {
    let trimmed = raw.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    serde_json::from_str(json_str).map_err(|e| format!("invalid batch JSON: {e}"))
}

pub fn parse_profile_update_response(raw: &str) -> Result<DeepSeekProfileUpdateResponse, String> {
    let trimmed = raw.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    serde_json::from_str(json_str).map_err(|e| format!("invalid DeepSeek JSON: {e}"))
}

fn parse_alias_expansion_response(
    raw: &str,
) -> Result<(Vec<DeepSeekAliasProposal>, String), String> {
    let trimmed = raw.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let value: Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid alias JSON: {e}"))?;
    let proposals: Vec<DeepSeekAliasProposal> = serde_json::from_value(
        value
            .get("alias_proposals")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|e| format!("invalid alias proposals: {e}"))?;
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((proposals, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::updater::types::DeepSeekClassification;

    #[test]
    fn parses_minimal_no_learning_response() {
        let raw = r#"{"schema_version":1,"classification":"no_learning","confidence":0.1,"reason":"ambiguous"}"#;
        let parsed = parse_profile_update_response(raw).expect("parse");
        assert_eq!(parsed.classification, DeepSeekClassification::NoLearning);
    }

    #[test]
    fn parses_alias_expansion_response() {
        let raw = r#"{"alias_proposals":[{"source_phrase":"n 10","canonical_phrase":"n8n","term_type":"brand","proposal_status":"candidate","confidence":0.9,"evidence_count_delta":1,"reason":"heard form"}],"reason":"ok"}"#;
        let (aliases, reason) = parse_alias_expansion_response(raw).expect("parse");
        assert_eq!(reason, "ok");
        assert_eq!(aliases[0].source_phrase, "n 10");
    }
}
