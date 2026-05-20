//! LLM-driven learning analyzer (Learning Pipeline v2).
//!
//! Replaces the 10-stage deterministic pipeline with a single Groq LLM call
//! that analyzes the full before/after diff holistically.
//!
//! Given (transcript, polished, user_kept), the analyzer classifies ALL changes
//! between polished and user_kept into one of five categories:
//!
//!   - `stt_error`          — STT misheard a content word
//!   - `polish_error`       — STT was right, polish rewrote wrongly
//!   - `format_preference`  — user prefers a specific format (times, numbers, etc.)
//!   - `style_preference`   — synonym/grammar swap, both versions fine
//!   - `structural_rewrite` — user added/rewrote entire sections
//!
//! For learnable changes (stt_error, polish_error, format_preference), the
//! analyzer drafts a meaning from context and signals whether to learn.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const ANALYZER_MODEL: &str = "llama-3.1-8b-instant";

// ── Public types ──────────────────────────────────────────────────────────────

/// Input to the analyzer LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerInput {
    pub transcript: String,
    pub polished: String,
    pub user_kept: String,
    pub output_language: String,
    pub existing_vocab: Vec<ExistingTerm>,
}

/// A vocabulary term the user already has — passed to the LLM so it can
/// reason about whether a correction is novel or a repeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingTerm {
    pub term: String,
    pub current_meaning: Option<String>,
    pub sighting_count: i64,
    pub examples: Vec<String>,
}

/// Structured output from the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerOutput {
    pub changes: Vec<AnalyzedChange>,
    pub overall_class: String,
}

/// A single change identified by the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzedChange {
    pub original: String,
    pub corrected: String,
    pub reason: ChangeReason,
    pub meaning: Option<String>,
    pub context_example: Option<String>,
    pub should_learn: bool,
    pub confidence: f64,
    pub skip_reason: Option<String>,
    pub format_rule: Option<String>,
}

/// Why the user made this change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    SttError,
    PolishError,
    FormatPreference,
    StylePreference,
    StructuralRewrite,
}

impl ChangeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SttError => "stt_error",
            Self::PolishError => "polish_error",
            Self::FormatPreference => "format_preference",
            Self::StylePreference => "style_preference",
            Self::StructuralRewrite => "structural_rewrite",
        }
    }

    pub fn is_learnable(&self) -> bool {
        matches!(
            self,
            Self::SttError | Self::PolishError | Self::FormatPreference
        )
    }
}

// ── System prompt ─────────────────────────────────────────────────────────────

fn build_system_prompt(output_language: &str, existing_vocab: &[ExistingTerm]) -> String {
    let stop_words = match output_language {
        "hindi" => STOP_WORDS_HINDI,
        "hinglish" => STOP_WORDS_HINGLISH,
        _ => STOP_WORDS_ENGLISH,
    };

    let vocab_context = if existing_vocab.is_empty() {
        String::new()
    } else {
        let entries: Vec<String> = existing_vocab
            .iter()
            .take(50)
            .map(|v| {
                let meaning_part = v
                    .current_meaning
                    .as_deref()
                    .map(|m| format!(" (current meaning: {m})"))
                    .unwrap_or_default();
                format!(
                    "  - \"{}\"{} — seen {}x",
                    v.term, meaning_part, v.sighting_count
                )
            })
            .collect();
        format!(
            "\n\nEXISTING VOCABULARY (already learned):\n{}\n\
             If a corrected term already exists here, set should_learn=false and skip_reason=\"already in vocabulary\".\n\
             If the existing meaning needs refinement based on new context, include the refined meaning in the `meaning` field.",
            entries.join("\n")
        )
    };

    format!(
        r#"You are a LEARNING ANALYZER for a voice dictation app. You analyze user edits to learn from corrections.

PIPELINE: user SPOKE → STT produced TRANSCRIPT → LLM produced POLISHED → user EDITED to USER_KEPT.

Your job:
1. Find ALL differences between POLISHED and USER_KEPT.
2. For each change, classify the reason into exactly one of 5 categories.
3. For learnable changes, draft a meaning from surrounding context.
4. Decide whether each change should be learned.

CHANGE REASONS (pick exactly one per change):

1. stt_error — STT misheard a content word. Both transcript and polish contain the wrong word; user_kept restores what was actually said. Common for acronyms (MACOBS, EMIAC), proper nouns (Vipassana), code identifiers (n8n), brand names (Claude).
   → should_learn=true (unless already in vocabulary)
   → Draft a meaning from the surrounding sentence context (1-2 sentences max)

   CRITICAL — distinguishing real STT errors from real words:
   The ORIGINAL word (what was in polished/transcript) must be GIBBERISH — a nonsense word that does NOT exist in Hindi, English, or Hinglish. Deepgram produces garbage like "amaix", "meah", "mccorb" when it cannot recognize a proper noun. Those are real STT errors.
   But if the original word IS a real Hindi/English word (maine, mac, abhi, dekho, tumne, hoga, kisko), then Deepgram transcribed CORRECTLY. The user is REPHRASING (replacing one real word with another), NOT correcting an STT error.
   ASK: "Does the original word exist as a real word in Hindi or English?"
     YES (mac, maine, mei, abhi, main) → style_preference or structural_rewrite, NOT stt_error
     NO (meah, amaix, mccorb, claud) → stt_error, safe to learn
   Learning real words as STT replacements is CATASTROPHIC — it replaces that common word everywhere in future dictation.

2. polish_error — STT got the word right, but the polish LLM rewrote it wrongly. User reverted to the transcript version.
   Signature: user_kept matches transcript but differs from polished.
   → should_learn=true

3. format_preference — User prefers a specific format for times/numbers/dates/units.
   Examples: "8am" → "8:00 AM", "10%" → "10 percent", "$100" → "100 dollars"
   → should_learn=true, set format_rule to describe the pattern

4. style_preference — Synonym swap, grammar tweak. Both versions are acceptable.
   Examples: "good" → "great", "utilize" → "use"
   → should_learn=false, skip_reason="style preference only"

5. structural_rewrite — User added new content, links, deleted sections, or rewrote substantially.
   → should_learn=false, skip_reason="structural rewrite"

FUNCTION WORD STOP-LIST — NEVER extract these as learnable terms:
{stop_words}

CRITICAL RULES FOR TERM EXTRACTION:
- Extract ONLY the single canonical word/name — NOT the phrase around it.
  WRONG: "Emiac Technologies" → should be just "EMIAC"
  WRONG: "Macobs kabj ko samajh lo bhai" → should be just "MACOBS"
  RIGHT: "MACOBS" (the specific term the user corrected)
  RIGHT: "EMIAC" (the specific brand name)
- Maximum 3 words per term. Multi-word terms are only OK for proper compound names like "Said App" or "n8n workflow".
- NEVER extract a full sentence or clause as a term.
- Use the CANONICAL casing the user typed (e.g., "MACOBS" not "macobs", "EMIAC" not "emiac").
- If the corrected word matches an EXISTING VOCABULARY term (case-insensitive), set should_learn=false and instead provide a REFINED meaning that incorporates this new context.
  Example: "EMIAC" exists with meaning "user's company". User now corrects "Technologies" → "EMIAC Technologies".
  → Return corrected="EMIAC", should_learn=false, skip_reason="already in vocabulary",
    meaning="EMIAC Technologies — the user's company, a technology firm"
  This refines the existing entry rather than creating a duplicate.

MEANING DRAFTING RULES:
- Draft meaning from the surrounding sentence context
- Keep to 1-2 sentences max
- Focus on WHAT the term is and HOW it's used
- If an existing meaning exists, DEEPEN it with new context (don't repeat, add new info)
- Examples of good meanings:
  Sighting 1: "MACOBS — possibly a stock ticker"
  Sighting 3: "MACOBS — Indian SME stock ticker on BSE/NSE, discussed in market-cap and IPO contexts"
  Sighting 7: "MACOBS — Indian SME stock ticker on BSE/NSE, used in valuation, IPO pricing, and quarterly results discussions"
{vocab_context}

OUTPUT — strict JSON only, no markdown, no commentary:
{{
  "changes": [
    {{
      "original": "what was in polished",
      "corrected": "what user changed it to",
      "reason": "stt_error|polish_error|format_preference|style_preference|structural_rewrite",
      "meaning": "1-2 sentence description or null",
      "context_example": "the sentence this appeared in or null",
      "should_learn": true,
      "confidence": 0.95,
      "skip_reason": null,
      "format_rule": null
    }}
  ],
  "overall_class": "the dominant reason across all changes"
}}

If there are NO differences between polished and user_kept, return:
{{"changes": [], "overall_class": "no_change"}}"#
    )
}

const STOP_WORDS_ENGLISH: &str = "the, a, an, is, was, are, were, and, or, but, in, on, at, to, for, of, with, that, this, it, he, she, they, we, I, my, your, his, her, its, our, their, do, does, did, have, has, had, will, would, could, should, can, may, might, been, being, am, not, no, so, if, then, than, just, also, very, too, only";

const STOP_WORDS_HINGLISH: &str = "the, a, an, is, was, are, were, and, or, but, in, on, at, to, for, of, with, that, this, it, he, she, they, we, I, my, your, his, her, its, our, their, do, does, did, have, has, had, will, would, could, should, can, may, might, been, being, am, not, no, so, if, then, than, just, also, very, too, only, ka, ke, ki, ko, se, mein, hai, hain, tha, the, thi, aur, ya, par, pe, ne, woh, yeh, kya, kaise, kab, kahan, hum, tum, main, iska, uska, apna, apni, apne, jo, jab, tab, abhi, bahut, bohot, nahi, nhi, bhi, sab, kuch, ek, do";

const STOP_WORDS_HINDI: &str = "का, के, की, को, से, में, है, हैं, था, थे, थी, और, या, पर, पे, ने, वो, ये, क्या, कैसे, कब, कहाँ, हम, तुम, मैं, इसका, उसका, अपना, जो, जब, तब, अभी, बहुत, नहीं, भी, सब, कुछ, एक, दो";

// ── Main entry point ──────────────────────────────────────────────────────────

/// Analyze a user edit via Groq LLM. Returns structured analysis of all changes
/// between polished output and what the user kept.
///
/// Returns `Err` on API/parse failures so the caller can decide how to handle
/// (typically: skip learning gracefully, same as the old classifier).
pub async fn analyze_edit(
    http: &Client,
    api_key: &str,
    codex_access_token: Option<&str>,
    input: &AnalyzerInput,
) -> Result<AnalyzerOutput, String> {
    if api_key.is_empty() && codex_access_token.map(|t| !t.is_empty()).unwrap_or(false) == false {
        return Err("no Groq API key and no Codex token — skipping analysis".into());
    }

    // Short-circuit if polished == user_kept (no edit).
    if input.polished.trim() == input.user_kept.trim() {
        return Ok(AnalyzerOutput {
            changes: vec![],
            overall_class: "no_change".into(),
        });
    }

    info!(
        "[analyzer] model={} (codex={})",
        if codex_access_token.map(|t| !t.is_empty()).unwrap_or(false) { "gpt-5.4-mini" } else { ANALYZER_MODEL },
        codex_access_token.is_some()
    );

    let system_prompt = build_system_prompt(&input.output_language, &input.existing_vocab);

    let user_message = format!(
        "<transcript>\n{}\n</transcript>\n\n\
         <polished>\n{}\n</polished>\n\n\
         <user_kept>\n{}\n</user_kept>\n\n\
         <output_language>{}</output_language>\n\n\
         Analyze all differences between polished and user_kept. \
         Return the analysis as strict JSON.",
        input.transcript, input.polished, input.user_kept, input.output_language,
    );

    // Try Codex (GPT-5.4-mini) first — smarter model makes better learning
    // decisions (real Hindi word vs gibberish STT output). Falls back to
    // Groq 8B if Codex token unavailable or call fails.
    if let Some(token) = codex_access_token {
        if !token.is_empty() {
            let start = std::time::Instant::now();
            info!("[analyzer] trying Codex gpt-5.4-mini for learning analysis");
            match super::openai_codex::call_json(
                http,
                token,
                super::openai_codex::MODEL_MINI,
                &system_prompt,
                &user_message,
            )
            .await
            {
                Ok(raw) => {
                    let ms = start.elapsed().as_millis();
                    let content = raw
                        .trim_start_matches("```json")
                        .trim_start_matches("```")
                        .trim_end_matches("```")
                        .trim();
                    if !content.is_empty() {
                        match serde_json::from_str::<AnalyzerOutput>(content) {
                            Ok(output) => {
                                info!(
                                    "[analyzer] codex {ms}ms — {} change(s), overall={}",
                                    output.changes.len(),
                                    output.overall_class,
                                );
                                return Ok(output);
                            }
                            Err(e) => {
                                warn!("[analyzer] codex JSON parse failed: {e} — falling back to Groq");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[analyzer] codex failed: {e} — falling back to Groq");
                }
            }
        }
    }

    // Fallback: Groq llama-3.1-8b-instant
    if api_key.is_empty() {
        return Err("Codex failed and no Groq API key — skipping analysis".into());
    }
    let body = json!({
        "model":           ANALYZER_MODEL,
        "temperature":     0.1,
        "max_tokens":      1024,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    let start = std::time::Instant::now();
    info!("[analyzer] fallback POST {GROQ_ENDPOINT} model={ANALYZER_MODEL}");

    let resp = http
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
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

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("response parse error: {e}"))?;

    let ms = start.elapsed().as_millis();

    let content = resp_json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    if content.is_empty() {
        return Err("empty response content".into());
    }

    let output = parse_analyzer_response(content)?;

    info!(
        "[analyzer] {ms}ms — overall={} changes={} learnable={}",
        output.overall_class,
        output.changes.len(),
        output.changes.iter().filter(|c| c.should_learn).count(),
    );

    Ok(output)
}

// ── Response parsing ──────────────────────────────────────────────────────────

/// Parse the LLM's JSON response into an `AnalyzerOutput`. Handles partial
/// results gracefully: if individual changes fail to parse, they're skipped
/// rather than failing the whole response.
pub fn parse_analyzer_response(s: &str) -> Result<AnalyzerOutput, String> {
    // Try full parse first.
    #[derive(Deserialize)]
    struct RawOutput {
        #[serde(default)]
        changes: Vec<serde_json::Value>,
        #[serde(default)]
        overall_class: String,
    }

    let raw: RawOutput = serde_json::from_str(s).map_err(|e| format!("JSON parse error: {e}"))?;

    let mut changes = Vec::new();
    for raw_change in &raw.changes {
        match parse_single_change(raw_change) {
            Ok(change) => changes.push(change),
            Err(e) => {
                warn!("[analyzer] skipping malformed change: {e}");
            }
        }
    }

    // Derive overall_class from changes if not provided.
    let overall_class = if !raw.overall_class.is_empty() {
        raw.overall_class
    } else if let Some(dominant) = dominant_class(&changes) {
        dominant
    } else {
        "no_change".into()
    };

    Ok(AnalyzerOutput {
        changes,
        overall_class,
    })
}

fn parse_single_change(v: &serde_json::Value) -> Result<AnalyzedChange, String> {
    #[derive(Deserialize)]
    struct RawChange {
        #[serde(default)]
        original: String,
        #[serde(default)]
        corrected: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        meaning: Option<String>,
        #[serde(default)]
        context_example: Option<String>,
        #[serde(default)]
        should_learn: bool,
        #[serde(default)]
        confidence: f64,
        #[serde(default)]
        skip_reason: Option<String>,
        #[serde(default)]
        format_rule: Option<String>,
    }

    let raw: RawChange =
        serde_json::from_value(v.clone()).map_err(|e| format!("change parse error: {e}"))?;

    let reason = match raw.reason.as_str() {
        "stt_error" => ChangeReason::SttError,
        "polish_error" => ChangeReason::PolishError,
        "format_preference" => ChangeReason::FormatPreference,
        "style_preference" => ChangeReason::StylePreference,
        "structural_rewrite" => ChangeReason::StructuralRewrite,
        other => return Err(format!("unknown reason: {other}")),
    };

    Ok(AnalyzedChange {
        original: raw.original,
        corrected: raw.corrected,
        reason,
        meaning: raw.meaning.filter(|s| !s.is_empty()),
        context_example: raw.context_example.filter(|s| !s.is_empty()),
        should_learn: raw.should_learn,
        confidence: raw.confidence.clamp(0.0, 1.0),
        skip_reason: raw.skip_reason.filter(|s| !s.is_empty()),
        format_rule: raw.format_rule.filter(|s| !s.is_empty()),
    })
}

/// Derive the dominant class with priority:
/// stt_error > polish_error > format_preference > style_preference > structural_rewrite
fn dominant_class(changes: &[AnalyzedChange]) -> Option<String> {
    fn priority(r: &ChangeReason) -> u8 {
        match r {
            ChangeReason::SttError => 5,
            ChangeReason::PolishError => 4,
            ChangeReason::FormatPreference => 3,
            ChangeReason::StylePreference => 2,
            ChangeReason::StructuralRewrite => 1,
        }
    }

    changes
        .iter()
        .max_by_key(|c| priority(&c.reason))
        .map(|c| c.reason.as_str().to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stt_error_change() {
        let json = r#"{
            "changes": [
                {
                    "original": "main course",
                    "corrected": "MACOBS",
                    "reason": "stt_error",
                    "meaning": "Indian SME stock ticker",
                    "context_example": "MACOBS ka IPO batana",
                    "should_learn": true,
                    "confidence": 0.95,
                    "skip_reason": null,
                    "format_rule": null
                }
            ],
            "overall_class": "stt_error"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(output.overall_class, "stt_error");
        assert_eq!(output.changes.len(), 1);
        assert_eq!(output.changes[0].reason, ChangeReason::SttError);
        assert_eq!(output.changes[0].corrected, "MACOBS");
        assert!(output.changes[0].should_learn);
        assert!(output.changes[0].meaning.is_some());
    }

    #[test]
    fn parse_polish_error_change() {
        let json = r#"{
            "changes": [
                {
                    "original": "kindly",
                    "corrected": "please",
                    "reason": "polish_error",
                    "meaning": null,
                    "context_example": null,
                    "should_learn": true,
                    "confidence": 0.85,
                    "skip_reason": null,
                    "format_rule": null
                }
            ],
            "overall_class": "polish_error"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(output.changes[0].reason, ChangeReason::PolishError);
        assert!(output.changes[0].should_learn);
    }

    #[test]
    fn parse_format_preference() {
        let json = r#"{
            "changes": [
                {
                    "original": "8am",
                    "corrected": "8:00 AM",
                    "reason": "format_preference",
                    "meaning": null,
                    "context_example": null,
                    "should_learn": true,
                    "confidence": 0.9,
                    "skip_reason": null,
                    "format_rule": "time: prefer HH:MM AM/PM format"
                }
            ],
            "overall_class": "format_preference"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(output.changes[0].reason, ChangeReason::FormatPreference);
        assert!(output.changes[0].format_rule.is_some());
    }

    #[test]
    fn parse_style_preference_not_learned() {
        let json = r#"{
            "changes": [
                {
                    "original": "good",
                    "corrected": "great",
                    "reason": "style_preference",
                    "meaning": null,
                    "context_example": null,
                    "should_learn": false,
                    "confidence": 0.7,
                    "skip_reason": "style preference only",
                    "format_rule": null
                }
            ],
            "overall_class": "style_preference"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert!(!output.changes[0].should_learn);
        assert!(output.changes[0].skip_reason.is_some());
    }

    #[test]
    fn parse_empty_changes() {
        let json = r#"{"changes": [], "overall_class": "no_change"}"#;
        let output = parse_analyzer_response(json).unwrap();
        assert!(output.changes.is_empty());
        assert_eq!(output.overall_class, "no_change");
    }

    #[test]
    fn parse_multiple_changes() {
        let json = r#"{
            "changes": [
                {
                    "original": "main course",
                    "corrected": "MACOBS",
                    "reason": "stt_error",
                    "meaning": "Stock ticker",
                    "should_learn": true,
                    "confidence": 0.9,
                    "context_example": null,
                    "skip_reason": null,
                    "format_rule": null
                },
                {
                    "original": "good",
                    "corrected": "great",
                    "reason": "style_preference",
                    "meaning": null,
                    "should_learn": false,
                    "confidence": 0.6,
                    "context_example": null,
                    "skip_reason": "style preference only",
                    "format_rule": null
                }
            ],
            "overall_class": "stt_error"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(output.changes.len(), 2);
        assert_eq!(output.overall_class, "stt_error");
    }

    #[test]
    fn parse_overall_class_inferred_when_missing() {
        let json = r#"{
            "changes": [
                {
                    "original": "kindly",
                    "corrected": "please",
                    "reason": "polish_error",
                    "should_learn": true,
                    "confidence": 0.85
                },
                {
                    "original": "main course",
                    "corrected": "MACOBS",
                    "reason": "stt_error",
                    "should_learn": true,
                    "confidence": 0.9
                }
            ]
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(
            output.overall_class, "stt_error",
            "stt_error should dominate over polish_error"
        );
    }

    #[test]
    fn parse_partial_changes_skips_invalid() {
        let json = r#"{
            "changes": [
                {
                    "original": "main course",
                    "corrected": "MACOBS",
                    "reason": "stt_error",
                    "should_learn": true,
                    "confidence": 0.9
                },
                {
                    "original": "bad",
                    "corrected": "bad",
                    "reason": "INVALID_REASON",
                    "should_learn": false,
                    "confidence": 0.1
                }
            ],
            "overall_class": "stt_error"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert_eq!(output.changes.len(), 1, "invalid change should be skipped");
        assert_eq!(output.changes[0].corrected, "MACOBS");
    }

    #[test]
    fn parse_confidence_clamped() {
        let json = r#"{
            "changes": [
                {
                    "original": "a",
                    "corrected": "b",
                    "reason": "stt_error",
                    "should_learn": true,
                    "confidence": 2.5
                }
            ],
            "overall_class": "stt_error"
        }"#;
        let output = parse_analyzer_response(json).unwrap();
        assert!(output.changes[0].confidence <= 1.0);
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let bad = "not json at all";
        assert!(parse_analyzer_response(bad).is_err());
    }

    #[test]
    fn change_reason_learnable() {
        assert!(ChangeReason::SttError.is_learnable());
        assert!(ChangeReason::PolishError.is_learnable());
        assert!(ChangeReason::FormatPreference.is_learnable());
        assert!(!ChangeReason::StylePreference.is_learnable());
        assert!(!ChangeReason::StructuralRewrite.is_learnable());
    }

    #[test]
    fn system_prompt_includes_stop_words_for_language() {
        let prompt_en = build_system_prompt("english", &[]);
        assert!(prompt_en.contains("the, a, an, is"));
        assert!(!prompt_en.contains("ka, ke, ki, ko"));

        let prompt_hi = build_system_prompt("hinglish", &[]);
        assert!(prompt_hi.contains("ka, ke, ki, ko"));
        assert!(prompt_hi.contains("the, a, an, is"));

        let prompt_hindi = build_system_prompt("hindi", &[]);
        assert!(prompt_hindi.contains("का, के, की, को"));
    }

    #[test]
    fn system_prompt_includes_existing_vocab() {
        let vocab = vec![ExistingTerm {
            term: "MACOBS".into(),
            current_meaning: Some("Indian SME stock ticker".into()),
            sighting_count: 3,
            examples: vec!["MACOBS ka IPO".into()],
        }];
        let prompt = build_system_prompt("english", &vocab);
        assert!(prompt.contains("MACOBS"));
        assert!(prompt.contains("Indian SME stock ticker"));
        assert!(prompt.contains("seen 3x"));
        assert!(prompt.contains("already in vocabulary"));
    }

    #[test]
    fn system_prompt_empty_vocab() {
        let prompt = build_system_prompt("english", &[]);
        assert!(!prompt.contains("EXISTING VOCABULARY (already learned):"));
    }
}
