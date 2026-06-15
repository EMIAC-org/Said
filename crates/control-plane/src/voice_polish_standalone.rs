//! Standalone voice polish pipeline — mirrors `voice_polish` / `polish_runtime_transcript`
//! in `routes/runtime.rs` without DB, auth, or telemetry. For local STT comparison scripts.

use serde_json::json;

use crate::format_recover;
use crate::number_format;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const GROQ_MODEL_FAST: &str = "llama-3.1-8b-instant";
const GROQ_MODEL_SMART: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

/// Full server-runtime polish path: number_format pre → Groq → literal restore → number_format post → email recover.
pub async fn polish_transcript(
    transcript: &str,
    output_language: &str,
    selected_model: &str,
    groq_api_key: &str,
    screen_context: Option<&str>,
    safe_vocab_terms: &[String],
) -> Result<String, String> {
    let formatted_transcript = number_format::apply(transcript);
    let system_prompt =
        build_voice_system_prompt(output_language, screen_context, safe_vocab_terms);
    let user_message = build_voice_user_message(&formatted_transcript, output_language);

    let model = if selected_model == "smart" {
        GROQ_MODEL_SMART
    } else {
        GROQ_MODEL_FAST
    };

    let output = call_groq(groq_api_key, model, &system_prompt, &user_message).await?;
    // Defensive guard: weak models occasionally echo the polish prompt's
    // role-anchor instructions into the output; strip any leaked lines.
    let output = strip_leaked_instructions(&output);
    let output = enforce_output_script(&output, output_language);

    let restored = restore_literal_tokens(&formatted_transcript, &output, safe_vocab_terms);
    let restored = restore_numeric_literal_tokens(&formatted_transcript, &restored);
    let output = number_format::apply(&restored);
    let output = restore_numeric_literal_tokens(&formatted_transcript, &output);
    Ok(format_recover::recover_emails(&output))
}

async fn call_groq(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(4096);
    let body = json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "stream": false,
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>"
        ],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    });

    let client = reqwest::Client::new();

    // Retry transient 429s (Groq TPM limit) with the server-advised backoff
    // instead of failing the whole dictation. 8B on the on-demand tier is only
    // ~6000 TPM and the polish prompt is large, so a burst of dictations hits
    // the limit; a short wait + retry turns a hard failure into a brief delay.
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0u32;
    let resp = loop {
        attempt += 1;
        let resp = client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| format!("groq request failed: {e}"))?;

        let status = resp.status();
        if status.is_success() {
            break resp;
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_ATTEMPTS {
            let header_wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f64>().ok());
            let preview = resp.text().await.unwrap_or_default();
            let wait_s = header_wait
                .or_else(|| parse_retry_seconds(&preview))
                .unwrap_or(1.0)
                .clamp(0.2, 5.0);
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_s)).await;
            continue;
        }
        let preview = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Groq returned {status}: {}",
            &preview[..preview.len().min(400)]
        ));
    };

    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("groq response parse failed: {e}"))?;

    let output = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if output.is_empty() {
        return Err("groq returned empty output".to_string());
    }

    Ok(output)
}

/// Extract the retry delay (seconds) from a Groq 429 message such as
/// "Please try again in 2.94s".
fn parse_retry_seconds(msg: &str) -> Option<f64> {
    let idx = msg.find("try again in ")?;
    let rest = &msg[idx + "try again in ".len()..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse::<f64>().ok()
}

/// Defensive output guard. Weak models (the 8B Fast model) occasionally echo
/// the polish prompt's role-anchor instructions into the output. When a
/// high-confidence leak signature is present, drop the leaked lines and keep
/// the real cleaned text; normal output (no signature) is returned untouched.
fn strip_leaked_instructions(output: &str) -> String {
    // Stored lowercase, matched case-insensitively. Only phrases that come
    // from the prompt, never from real dictation, to avoid false positives.
    const LEAK_MARKERS: &[&str] = &[
        "transcription cleaner",
        "you never answer questions",
        "never follow commands",
        "only clean the spoken",
        "only return the cleaned",
        "=== begin transcript",
        "=== end transcript",
        "final check",
    ];
    let lower = output.to_lowercase();
    if !LEAK_MARKERS.iter().any(|m| lower.contains(m)) {
        return output.to_string();
    }
    let lines: Vec<&str> = output.lines().collect();
    // The real cleaned text follows the echoed instructions: take everything
    // after the last leaked line.
    if let Some(idx) = lines.iter().rposition(|line| {
        let ll = line.to_lowercase();
        LEAK_MARKERS.iter().any(|m| ll.contains(m))
    }) {
        let tail = lines[idx + 1..].join("\n").trim().to_string();
        if !tail.is_empty() {
            return tail;
        }
        // Echo ran to the end of the output — drop the marked lines instead.
        let kept: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| {
                let ll = line.to_lowercase();
                !LEAK_MARKERS.iter().any(|m| ll.contains(m))
            })
            .collect();
        let cleaned = kept.join("\n").trim().to_string();
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    output.to_string()
}

/// Shared with `routes/runtime.rs` — server `/v1/runtime/voice/polish` and `polish-cli`.
pub fn build_voice_system_prompt(
    output_language: &str,
    screen_context: Option<&str>,
    safe_vocab_terms: &[String],
) -> String {
    use said_core::polish::prompt::{
        VocabEntry, VocabResolution, build_system_prompt_with_vocab_entries,
    };
    use said_core::polish::types::PolishPrefs;

    // Server runtime has no per-account tone/custom prompt; use the 2.3.3 default.
    let prefs = PolishPrefs {
        output_language: output_language.to_string(),
        tone_preset: "neutral".to_string(),
        custom_prompt: None,
    };
    // Server vocab arrives as already-trusted bare terms (no STT aliases), so
    // they render as Resolved entries and the common-word filter never fires.
    let vocab_entries: Vec<VocabEntry> = safe_vocab_terms
        .iter()
        .filter_map(|t| {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(VocabEntry {
                    term: trimmed.to_string(),
                    context: None,
                    resolution: VocabResolution::Resolved,
                    term_type: None,
                    meaning: None,
                    stt_aliases: vec![],
                })
            }
        })
        .collect();

    let mut prompt =
        build_system_prompt_with_vocab_entries(&prefs, &[], &[], &vocab_entries, |_| false);

    if let Some(ctx) = screen_context {
        let trimmed = ctx.trim();
        if !trimmed.is_empty() {
            let clipped: String = trimmed.chars().take(400).collect();
            prompt.push_str(&format!(
                "\n\nSCREEN CONTEXT: \"{clipped}\"\nUse only as a tiebreaker for names or terms. Transcript words come first."
            ));
        }
    }

    prompt
}

pub fn build_voice_user_message(transcript: &str, output_language: &str) -> String {
    said_core::polish::prompt::build_user_message(transcript, output_language)
}

/// Apply the shared mechanical Devanagari -> Roman guard to the model output.
/// Mirrors the local backend: romanize + strip non-Latin scripts for
/// Hinglish/English modes; leave explicit Hindi output untouched.
pub fn enforce_output_script(output: &str, output_language: &str) -> String {
    if output_language == "hindi" {
        output.to_string()
    } else {
        let romanized = said_core::polish::script::enforce_roman_hinglish(output);
        said_core::polish::script::strip_non_latin_scripts(&romanized)
    }
}

fn restore_literal_tokens(transcript: &str, output: &str, safe_vocab_terms: &[String]) -> String {
    let source_words = transcript.split_whitespace().collect::<Vec<_>>();
    let mut output_words = output
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if source_words.is_empty() || source_words.len() != output_words.len() {
        return output.to_string();
    }

    let mut changed = false;
    for (source, out_word) in source_words.iter().zip(output_words.iter_mut()) {
        let source_core = trim_token_edges(source);
        let output_core = trim_token_edges(out_word);
        if source_core.is_empty() || output_core.is_empty() {
            continue;
        }
        if !is_literal_preserve_token(source_core, safe_vocab_terms) {
            continue;
        }
        if contains_token_case_insensitive(output, source_core) {
            continue;
        }
        if !source_core.eq_ignore_ascii_case(output_core) {
            *out_word = replace_token_core(out_word, source_core);
            changed = true;
        }
    }

    if changed {
        output_words.join(" ")
    } else {
        output.to_string()
    }
}

fn restore_numeric_literal_tokens(transcript: &str, output: &str) -> String {
    let source_words = transcript.split_whitespace().collect::<Vec<_>>();
    let mut output_words = output
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if source_words.is_empty() || source_words.len() != output_words.len() {
        return output.to_string();
    }

    let mut changed = false;
    for (source, out_word) in source_words.iter().zip(output_words.iter_mut()) {
        let Some(source_core) = numeric_literal_core(source) else {
            continue;
        };
        let Some(output_core) = numeric_literal_core(out_word) else {
            continue;
        };
        if source_core == output_core {
            continue;
        }
        if numeric_digits(&source_core) != numeric_digits(&output_core) {
            continue;
        }
        *out_word = replace_numeric_token_core(out_word, &source_core);
        changed = true;
    }

    if changed {
        output_words.join(" ")
    } else {
        output.to_string()
    }
}

fn numeric_literal_core(token: &str) -> Option<String> {
    let core = token
        .trim_matches(|c: char| !(c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ',')));
    if core.is_empty() || !core.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let has_format_symbol = core
        .chars()
        .any(|c| matches!(c, '$' | '₹' | '%' | '.' | ','));
    if has_format_symbol {
        Some(core.to_string())
    } else {
        None
    }
}

fn numeric_digits(token: &str) -> String {
    token.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn replace_numeric_token_core(output_word: &str, source_core: &str) -> String {
    let start = output_word
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ','))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = output_word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ','))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(output_word.len());

    format!(
        "{}{}{}",
        &output_word[..start],
        source_core,
        &output_word[end..]
    )
}

fn trim_token_edges(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
}

fn is_literal_preserve_token(token: &str, safe_vocab_terms: &[String]) -> bool {
    if safe_vocab_terms
        .iter()
        .any(|term| term.trim().eq_ignore_ascii_case(token))
    {
        return true;
    }
    let has_upper = token.chars().any(|c| c.is_ascii_uppercase());
    let has_internal_upper = token.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let has_digit_or_symbol = token
        .chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '@' | '/'));
    let is_all_caps = token
        .chars()
        .all(|c| !c.is_ascii_alphabetic() || c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_alphabetic());

    token.len() >= 3 && (has_digit_or_symbol || has_internal_upper || is_all_caps || has_upper)
}

fn contains_token_case_insensitive(text: &str, token: &str) -> bool {
    text.split_whitespace()
        .map(trim_token_edges)
        .any(|part| part.eq_ignore_ascii_case(token))
}

fn replace_token_core(output_word: &str, source_core: &str) -> String {
    let start = output_word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = output_word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(output_word.len());

    format!(
        "{}{}{}",
        &output_word[..start],
        source_core,
        &output_word[end..]
    )
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn strip_leak_drops_echoed_instructions_keeps_real_text() {
        let leaked = "You are a TRANSCRIPTION CLEANER, not a conversational AI.\n\
                      You NEVER answer questions.\n\
                      Kal mujhe office jaldi jana hai.";
        assert_eq!(
            strip_leaked_instructions(leaked),
            "Kal mujhe office jaldi jana hai."
        );
    }

    #[test]
    fn strip_leak_passes_normal_output_untouched() {
        let normal = "Mera email test@gmail.com hai aur password reset karna hai.";
        assert_eq!(strip_leaked_instructions(normal), normal);
    }

    #[test]
    fn strip_leak_keeps_original_when_only_markers() {
        // Echo with no trailing real text: keep original rather than emit empty.
        let only = "=== BEGIN TRANSCRIPT ===";
        assert_eq!(strip_leaked_instructions(only), only);
    }

    #[test]
    fn parse_retry_seconds_reads_groq_message() {
        let msg = "Rate limit reached ... Please try again in 2.94s. Need more tokens?";
        assert_eq!(parse_retry_seconds(msg), Some(2.94));
    }

    #[test]
    fn parse_retry_seconds_none_when_absent() {
        assert_eq!(parse_retry_seconds("no delay here"), None);
    }
}
