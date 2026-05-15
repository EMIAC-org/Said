//! Fast Groq LLM call to convert Devanagari output to natural Roman Hinglish.
//!
//! When the primary polish LLM emits Devanagari despite prompt instructions,
//! this recovery call produces natural Hinglish instead of the mechanical
//! character-by-character transliteration from `script::romanize_devanagari`.

use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const RECOVERY_MODEL: &str = "llama-3.1-8b-instant";
const RECOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

const SYSTEM_PROMPT: &str = "\
You are a script converter. Convert the input text from Devanagari to Roman Hinglish.

RULES:
- Convert ALL Devanagari words to their natural Roman/Latin script equivalent.
- Keep English words exactly as they are.
- Do NOT translate Hindi to English. Keep the Hindi words, just write them in Roman script.
- Use natural, commonly-used Hinglish spellings (e.g. \"chahiye\" not \"chaahie\", \"bhi\" not \"bhee\", \"hai\" not \"hai\").
- Preserve punctuation, capitalization of English words, numbers, and sentence structure.
- Output ONLY the converted text. No explanation, no preamble.

Examples:
Input: यह बहुत अच्छा है yaar, let's do this.
Output: Yeh bahut accha hai yaar, let's do this.

Input: मैं कल आऊंगा, please confirm.
Output: Main kal aaunga, please confirm.

Input: इसको check कर लो please.
Output: Isko check kar lo please.";

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}
#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}
#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
}

pub async fn recover(client: &Client, groq_api_key: &str, text: &str) -> Result<String, String> {
    if groq_api_key.is_empty() {
        return Err("no Groq key".into());
    }

    let body = json!({
        "model": RECOVERY_MODEL,
        "temperature": 0.0,
        "max_tokens": (text.len() / 2 + 256).min(4096),
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": text },
        ]
    });

    let start = std::time::Instant::now();
    info!(
        "[devanagari-recovery] POST {GROQ_ENDPOINT} model={RECOVERY_MODEL} input_chars={}",
        text.len()
    );

    let resp = client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {groq_api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(RECOVERY_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            &body_text[..body_text.len().min(300)]
        ));
    }

    let resp_json: ChatResponse = resp.json().await.map_err(|e| format!("parse error: {e}"))?;

    let content = resp_json
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();

    let ms = start.elapsed().as_millis();
    info!(
        "[devanagari-recovery] done in {ms}ms — {} → {} chars",
        text.len(),
        content.len()
    );

    if content.is_empty() {
        return Err("empty response".into());
    }

    if super::script::contains_devanagari(&content) {
        warn!(
            "[devanagari-recovery] LLM recovery still contains Devanagari — will fall back to mechanical romanization"
        );
        return Err("recovery output still contains Devanagari".into());
    }

    Ok(content)
}
