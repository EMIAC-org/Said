//! Four-way edit classifier (Groq llama-3.1-8b-instant, ~150 ms).
//!
//! Given (transcript, ai_output, user_kept), assign the edit to exactly one of:
//!
//!   • `STT_ERROR`     — STT mis-heard a word the user actually said. The user-
//!                       final form is correct; transcript and polish are wrong.
//!                       Promote to STT-layer vocabulary / phonetic replacement.
//!
//!   • `POLISH_ERROR`  — STT got the word right; polish rewrote it wrongly. The
//!                       user reverted to the transcript word. Promote to LLM
//!                       polish corrections.
//!
//!   • `USER_REPHRASE` — User changed a word that was correctly transcribed and
//!                       correctly polished — they just prefer different wording.
//!                       Do not learn (or learn weakly into style RAG only).
//!
//!   • `USER_REWRITE`  — Large-scale rewrite (>50 % of words changed) or user
//!                       added new context. Do not learn.
//!
//! KEY INVARIANT (the bug the old binary classifier had):
//!     The transcript is **NOT** ground truth. STT errors are exactly the case
//!     where transcript and polish agree on the wrong word.  When the user-final
//!     token looks jargon-like (digits, mixed case, rare term) and is missing
//!     from the transcript, prefer `STT_ERROR` over `USER_REPHRASE`.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use super::edit_diff::Hunk;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const CLASSIFIER_MODEL: &str = "llama-3.1-8b-instant";

/// Specific token-level correction extracted from inside a composite hunk.
///
/// When the user's edit bundles multiple changes (e.g. fixed a misheard name
/// AND wrapped it in a markdown link), the diff produces ONE hunk but only a
/// SUB-string of it is the actual learnable STT/polish error.  The labeler
/// emits this struct to tell us "promote *this specific term* from inside the
/// hunk", rather than the whole `kept_window`.
///
/// Example:
///   hunk.polish_window = "Anis at the rate Gmail dot com"
///   hunk.kept_window   = "[anish@gmail.com](mailto:anish@gmail.com)"
///   extracted_term     = { transcript_form: "Anis", correct_form: "anish" }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedTerm {
    /// What STT actually wrote for this token (within the polish/transcript).
    pub transcript_form: String,
    /// The correctly-spelled form the user wanted (must be a whole-word
    /// substring of `hunk.kept_window`).
    pub correct_form: String,
}

/// One labelled hunk — pairs a deterministic diff hunk with the LLM's class
/// assignment AND (optionally) a specific token-level extraction within the
/// hunk.  Promotion uses `extracted_term` when present, otherwise falls back
/// to `kept_window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelledHunk {
    pub hunk: Hunk,
    pub class: EditClass,
    pub confidence: f64,
    /// When the LLM identifies a specific token within the hunk as the actual
    /// STT/polish error (vs. the whole hunk being the correction), it emits
    /// this field.  Stage-4 promotion prefers it over `kept_window`.
    #[serde(default)]
    pub extracted_term: Option<ExtractedTerm>,
}

/// Backwards-compatible alias for the route layer — a labelled hunk is the
/// candidate now.  The route consumes `transcript_form` / `polish_form` /
/// `correct_form` getters defined below.
pub type Candidate = LabelledHunk;

impl LabelledHunk {
    /// What STT transcribed for the candidate.  When `extracted_term` is
    /// present, returns the specific sub-token from inside the hunk; else
    /// returns the full hunk's transcript window.
    pub fn transcript_form(&self) -> &str {
        self.extracted_term
            .as_ref()
            .map(|t| t.transcript_form.as_str())
            .unwrap_or(&self.hunk.transcript_window)
    }
    /// What the polish step produced.  No extraction equivalent — polish_form
    /// is always the hunk's polish window because polish errors are the WHOLE
    /// substituted region.
    pub fn polish_form(&self) -> &str {
        &self.hunk.polish_window
    }
    /// The proposed correct form.  When `extracted_term` is present, returns
    /// the specific sub-token; else returns the full hunk's kept window.
    /// Stage-4 promotion uses THIS getter.
    pub fn correct_form(&self) -> &str {
        self.extracted_term
            .as_ref()
            .map(|t| t.correct_form.as_str())
            .unwrap_or(&self.hunk.kept_window)
    }
    /// Best guess at what was actually spoken.  For STT_ERROR this equals
    /// `correct_form()` (user restored what they actually said).
    pub fn spoke(&self) -> &str {
        self.correct_form()
    }
}

/// The four mutually-exclusive classes of edit.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EditClass {
    SttError,
    PolishError,
    UserRephrase,
    UserRewrite,
}

impl EditClass {
    pub fn as_str(self) -> &'static str {
        match self {
            EditClass::SttError => "STT_ERROR",
            EditClass::PolishError => "POLISH_ERROR",
            EditClass::UserRephrase => "USER_REPHRASE",
            EditClass::UserRewrite => "USER_REWRITE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "STT_ERROR" | "STTERROR" => Some(Self::SttError),
            "POLISH_ERROR" | "POLISHERROR" => Some(Self::PolishError),
            "USER_REPHRASE" | "USERREPHRASE" => Some(Self::UserRephrase),
            "USER_REWRITE" | "USERREWRITE" => Some(Self::UserRewrite),
            _ => None,
        }
    }

    /// Should this class produce any learning artifacts?
    pub fn is_learnable(self) -> bool {
        matches!(self, Self::SttError | Self::PolishError)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifyResult {
    pub class: EditClass,
    pub reason: String,
    /// One entry per diff hunk (in input order).  Each carries the original
    /// hunk plus the LLM's class label and confidence.  The route uses the
    /// hunk's text — it never relies on the LLM having "invented" a term.
    pub candidates: Vec<LabelledHunk>,
    /// Mean confidence across labelled hunks ∈ [0, 1].  Soft signal — promotion
    /// logic also uses script + phonetic + jargon gates as defense in depth.
    #[serde(default)]
    pub confidence: f64,
}

const SYSTEM_PROMPT: &str = r#"You are a LABELER. You assign one class label to each pre-extracted hunk of a user's post-dictation edit.

PIPELINE: user SPOKE audio → STT produced TRANSCRIPT → LLM produced POLISH → user EDITED to USER_KEPT.

A hunk is a contiguous region where polish and user_kept differ. Hunks are computed deterministically from the actual text — you DID NOT and CANNOT invent them. Each hunk gives you:
  • transcript_window — the corresponding region of the original transcript (may be empty if positional alignment failed)
  • polish_window     — what the polish step produced for this region (may be empty for pure insertions)
  • kept_window       — what the user actually kept for this region (may be empty for pure deletions)

For each hunk, pick exactly ONE class:

1. STT_ERROR — The STT engine misheard a word the user actually said. Both transcript and polish contain the wrong word; user_kept restores what was actually spoken. Most common for:
     • All-caps acronyms (NASA, IPO, MACOBS, FBI, EMIAC)
     • Brand / product names (Cursor, Claude, Linear, Anish)
     • Code identifiers (n8n, k8s, snake_case, camelCase)
     • Non-English / technical terms (Vipassana, satsang)
   Signature: kept_window is absent from transcript AND polish AND looks jargon-like (digits, mixed case, all-caps acronym, capitalised proper noun, rare term).  When polish has multiple common words and kept compresses them into a single jargon-y token (e.g. "Main corps" → "MACOBS", "Cloud Code" → "ClaudeCode"), this is virtually always STT_ERROR — set confidence high (≥ 0.85).

   CRITICAL — STT_ERROR vs legitimate Hindi/Hinglish words:
   The transcript_form in an STT_ERROR must be GIBBERISH — a word that does NOT exist in Hindi, English, or Hinglish. Deepgram produces nonsense like "amaix", "meah", "mccorb", "claud" when it cannot recognize a proper noun. These are legitimate STT errors worth learning.
   However, if the transcript_form is a REAL Hindi/Hinglish word that Deepgram correctly transcribed (maine, tumne, kisko, dekho, hoga, kaise, abhi, bhai, etc.), then the user is REPHRASING, not correcting an STT error. Deepgram heard "maine" correctly — the user just decided to replace that word with something else.
   ASK YOURSELF: "Does this transcript_form exist as a real word in Hindi or English?" If YES → USER_REPHRASE. If NO (it's gibberish/nonsense) → STT_ERROR.
   Example: transcript_window="meah", kept_window="Emiac" → STT_ERROR ("meah" is gibberish, Deepgram mangled "Emiac").
   Example: transcript_window="maine", kept_window="Emiac" → USER_REPHRASE ("maine" is Hindi for "I did", Deepgram heard correctly, user is replacing a real word).
   Example: transcript_window="amaix", kept_window="Emiac" → STT_ERROR ("amaix" is nonsense).
   Example: transcript_window="abhi", kept_window="Emiac" → USER_REPHRASE ("abhi" is Hindi for "now/currently", not an STT error).

   Example: transcript_window="written", polish_window="written", kept_window="n8n" → STT_ERROR (confidence 0.95).
   Example: transcript_window="Main corps", polish_window="Main corps", kept_window="MACOBS" → STT_ERROR (confidence 0.9), extracted_term: {"transcript_form":"Main corps","correct_form":"MACOBS"}.

2. POLISH_ERROR — STT captured correctly, polish rewrote wrongly, user reverted to transcript form.
   Signature: kept_window equals transcript_window AND differs from polish_window.
   Example: transcript_window="please", polish_window="kindly", kept_window="please" → POLISH_ERROR.

3. USER_REPHRASE — Stylistic preference swap. STT and polish are both reasonable; user just prefers different wording.
   Signature: kept_window is a common-language synonym of polish_window AND not jargon-like.
   Example: polish_window="good", kept_window="great" → USER_REPHRASE.

4. USER_REWRITE — Pure insertion (polish_window empty), pure deletion (kept_window empty), or large structural change.
   Example: polish_window="", kept_window="[anish@gmail.com](mailto:anish@gmail.com)" → USER_REWRITE.

CRITICAL RULES (DO NOT VIOLATE):
A. Use ONLY the text inside the hunks I gave you. NEVER invent words that don't appear in transcript / polish / kept windows.
B. If kept_window contains characters in a different script than the user's output_language preference, the answer is USER_REPHRASE — do NOT propose script translation as a learnable error.
C. The transcript may itself be wrong. STT_ERROR is exactly the case where transcript and polish both contain the same wrong word.
D. If polish_window is empty (pure insertion) → USER_REWRITE. If kept_window is empty (pure deletion) → USER_REWRITE.

EXTRACTED TERMS (important for composite hunks):
A hunk often bundles MULTIPLE changes (e.g. a misheard name + a markdown link wrapper). When the hunk's class is STT_ERROR or POLISH_ERROR and only a SUB-string of the hunk is the actual learnable correction, you MUST emit `extracted_term` to point at the specific tokens.

  • `extracted_term.transcript_form` MUST be a whole-word substring of `polish_window` (or `transcript_window` if polish_window is empty).
  • `extracted_term.correct_form`    MUST be a whole-word substring of `kept_window`.
  • Do NOT include surrounding punctuation, brackets, or formatting characters.
  • Do NOT invent text that isn't literally present in the windows.

When the hunk IS the correction in its entirety (e.g. polish_window="written", kept_window="n8n"), set extracted_term to {"transcript_form":"written","correct_form":"n8n"} — a single-word hunk's extracted term equals the hunk itself.

Examples:
  Hunk: polish="written" kept="n8n"
    → extracted_term: {"transcript_form":"written","correct_form":"n8n"}

  Hunk: polish="Anis at the rate Gmail dot com"
        kept  ="[anish@gmail.com](mailto:anish@gmail.com)"
    → STT_ERROR, extracted_term: {"transcript_form":"Anis","correct_form":"anish"}
    (The markdown link wrapping is REWRITE shape; only the name is the learnable STT fix.)

  Hunk: polish="kindly" kept="please"
    → POLISH_ERROR, extracted_term: {"transcript_form":"kindly","correct_form":"please"}

  Hunk: polish="" kept="[link](mailto:x@y)"
    → USER_REWRITE, extracted_term: null

OUTPUT — strict JSON only, no markdown, no commentary:
{
  "labels": [
    {
      "hunk_index": 0,
      "class": "STT_ERROR" | "POLISH_ERROR" | "USER_REPHRASE" | "USER_REWRITE",
      "confidence": 0.0,
      "extracted_term": {"transcript_form": "...", "correct_form": "..."} | null
    }
  ],
  "overall":    "STT_ERROR" | "POLISH_ERROR" | "USER_REPHRASE" | "USER_REWRITE",
  "reason":     "one sentence explanation referencing the actual hunk windows"
}

`overall` is the single dominant class for the whole edit (use the most-learnable class present, with priority STT_ERROR > POLISH_ERROR > USER_REPHRASE > USER_REWRITE).
`labels[i]` MUST exist for every hunk_index from 0 to N-1 in input order.
`confidence` ∈ [0,1]. Be honest — low when the hunk is ambiguous.
`extracted_term` is REQUIRED for STT_ERROR and POLISH_ERROR labels. Set to null only for USER_REPHRASE / USER_REWRITE."#;

/// Label diff-derived hunks via Groq.  Hunks are computed by `edit_diff::diff`
/// before this is called — the LLM only assigns class labels, it cannot
/// invent candidates.  Returns `None` if the API call fails (caller skips
/// learning gracefully).
///
/// `hunks` MUST be non-empty.  Empty hunks should be short-circuited at the
/// pre-filter stage (no learnable change → don't waste an API call).
pub async fn classify_edit(
    client: &Client,
    groq_api_key: &str,
    codex_access_token: Option<&str>,
    transcript: &str,
    ai_output: &str,
    user_kept: &str,
    hunks: &[Hunk],
    output_language: &str,
) -> Option<ClassifyResult> {
    if groq_api_key.is_empty() && codex_access_token.is_none() {
        warn!("[classifier] no API keys — skipping classification");
        return None;
    }
    if hunks.is_empty() {
        warn!("[classifier] called with zero hunks — refusing");
        return None;
    }

    info!(
        "[classifier] model={} (codex={})",
        if codex_access_token.map(|t| !t.is_empty()).unwrap_or(false) {
            "gpt-5.4-mini"
        } else {
            CLASSIFIER_MODEL
        },
        codex_access_token.is_some()
    );

    let hunks_block: String = hunks
        .iter()
        .enumerate()
        .map(|(i, h)| {
            format!(
                "  {{\"hunk_index\": {i}, \
                  \"transcript_window\": {tx:?}, \
                  \"polish_window\": {pw:?}, \
                  \"kept_window\": {kw:?}}}",
                tx = h.transcript_window,
                pw = h.polish_window,
                kw = h.kept_window,
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    let user_message = format!(
        "<output_language>{output_language}</output_language>\n\n\
         <transcript>\n{transcript}\n</transcript>\n\n\
         <polish>\n{ai_output}\n</polish>\n\n\
         <user_kept>\n{user_kept}\n</user_kept>\n\n\
         <hunks>\n[\n{hunks_block}\n]\n</hunks>\n\n\
         Label each hunk by hunk_index in input order. \
         Return overall + labels[] + reason as strict JSON."
    );

    // Try Codex (GPT-5.4-mini) first — smarter model, better at distinguishing
    // real Hindi words from STT gibberish. Fall back to Groq 8B if unavailable.
    if let Some(token) = codex_access_token {
        if !token.is_empty() {
            let start = std::time::Instant::now();
            info!("[classifier] trying Codex gpt-5.4-mini");
            match super::openai_codex::call_json(
                client,
                token,
                super::openai_codex::MODEL_MINI,
                SYSTEM_PROMPT,
                &user_message,
            )
            .await
            {
                Ok(content) => {
                    let ms = start.elapsed().as_millis();
                    let content = content
                        .trim_start_matches("```json")
                        .trim_start_matches("```")
                        .trim_end_matches("```")
                        .trim();
                    if !content.is_empty() {
                        if let Some(r) = parse_label_response(content, hunks) {
                            info!(
                                "[classifier] codex {ms}ms — overall={} labelled={}/{} conf={:.2} reason={:?}",
                                r.class.as_str(),
                                r.candidates.len(),
                                hunks.len(),
                                r.confidence,
                                r.reason,
                            );
                            return Some(r);
                        }
                        warn!("[classifier] codex returned unparseable JSON, falling back to Groq");
                    }
                }
                Err(e) => {
                    warn!("[classifier] codex failed: {e} — falling back to Groq");
                }
            }
        }
    }

    // Fallback: Groq llama-3.1-8b-instant
    if groq_api_key.is_empty() {
        warn!("[classifier] no Groq API key — skipping classification");
        return None;
    }

    let body = json!({
        "model":           CLASSIFIER_MODEL,
        "temperature":     0.0,
        "max_tokens":      600,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user",   "content": user_message  },
        ]
    });

    let start = std::time::Instant::now();
    info!("[classifier] fallback POST {GROQ_ENDPOINT} model={CLASSIFIER_MODEL}");

    let resp = match client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {groq_api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("[classifier] request failed: {e}");
            return None;
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        warn!(
            "[classifier] HTTP {status}: {}",
            &body_text[..body_text.len().min(300)]
        );
        return None;
    }

    let resp_json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            warn!("[classifier] parse error: {e}");
            return None;
        }
    };

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
        warn!("[classifier] empty response content");
        return None;
    }

    parse_label_response(content, hunks)
        .map(|r| {
            info!(
                "[classifier] groq {ms}ms — overall={} labelled={}/{} mean_conf={:.2} reason={:?}",
                r.class.as_str(),
                r.candidates.len(),
                hunks.len(),
                r.confidence,
                r.reason,
            );
            r
        })
        .or_else(|| {
            warn!("[classifier] failed to parse JSON: {content:?}");
            None
        })
}

/// True if `needle` appears as a whole-word substring of `haystack`.  Word
/// boundaries are defined by Unicode whitespace and ASCII punctuation (with
/// `_` and `-` treated as word characters so identifiers like `n8n-prod` are
/// still atomic).  Used to validate LLM-extracted terms — the term MUST be
/// literal text from the diff hunk's windows; we never trust an extraction
/// that doesn't survive this check.
fn is_whole_word_substring(needle: &str, haystack: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    haystack
        .split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '_' && c != '-'))
        .any(|tok| {
            let tok = tok.trim();
            !tok.is_empty()
                && (tok == needle
                    || (tok.is_ascii() && needle.is_ascii() && tok.eq_ignore_ascii_case(needle)))
        })
}

/// Parse the labeler response.  Pairs each LLM label with its corresponding
/// diff hunk by `hunk_index`.  If the LLM omits a hunk we treat its class
/// as USER_REPHRASE (safe default — no learning).
///
/// Tolerates minor schema drift: missing fields, lowercase enum values, and
/// the legacy invent-candidates schema (mapped to USER_REPHRASE since we
/// can't trust LLM-fabricated terms anyway).
pub fn parse_label_response(s: &str, hunks: &[Hunk]) -> Option<ClassifyResult> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        overall: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        labels: Vec<RawLabel>,
    }
    #[derive(Deserialize)]
    struct RawLabel {
        #[serde(default)]
        hunk_index: usize,
        #[serde(default)]
        class: String,
        #[serde(default)]
        confidence: f64,
        #[serde(default)]
        extracted_term: Option<RawExtracted>,
    }
    #[derive(Deserialize)]
    struct RawExtracted {
        #[serde(default)]
        transcript_form: String,
        #[serde(default)]
        correct_form: String,
    }

    let raw: Raw = serde_json::from_str(s).ok()?;
    // Priority: STT_ERROR > POLISH_ERROR > USER_REPHRASE > USER_REWRITE.
    fn priority(c: EditClass) -> u8 {
        match c {
            EditClass::SttError => 4,
            EditClass::PolishError => 3,
            EditClass::UserRephrase => 2,
            EditClass::UserRewrite => 1,
        }
    }
    let dominant_from_labels = || -> Option<EditClass> {
        let mut best: Option<EditClass> = None;
        for l in &raw.labels {
            let Some(c) = EditClass::parse(&l.class) else {
                continue;
            };
            if best.map(priority).unwrap_or(0) < priority(c) {
                best = Some(c);
            }
        }
        best
    };
    let overall = EditClass::parse(&raw.overall).or_else(dominant_from_labels)?;

    let mut labelled: Vec<LabelledHunk> = Vec::with_capacity(hunks.len());
    for (i, hunk) in hunks.iter().enumerate() {
        let lbl = raw.labels.iter().find(|l| l.hunk_index == i);
        let class = lbl
            .and_then(|l| EditClass::parse(&l.class))
            .unwrap_or(EditClass::UserRephrase);
        let confidence = lbl.map(|l| l.confidence.clamp(0.0, 1.0)).unwrap_or(0.0);

        // Validate extracted_term against the hunk's actual text — both forms
        // must be whole-word substrings of the corresponding window.  This
        // makes hallucination impossible at the schema level.
        let extracted_term = lbl.and_then(|l| l.extracted_term.as_ref()).and_then(|et| {
            let tf = et.transcript_form.trim();
            let cf = et.correct_form.trim();
            if tf.is_empty() || cf.is_empty() {
                return None;
            }
            let polish_haystack = if hunk.polish_window.is_empty() {
                &hunk.transcript_window
            } else {
                &hunk.polish_window
            };
            if !is_whole_word_substring(tf, polish_haystack) {
                return None;
            }
            if !is_whole_word_substring(cf, &hunk.kept_window) {
                return None;
            }
            Some(ExtractedTerm {
                transcript_form: tf.to_string(),
                correct_form: cf.to_string(),
            })
        });

        labelled.push(LabelledHunk {
            hunk: hunk.clone(),
            class,
            confidence,
            extracted_term,
        });
    }

    let mean_conf = if labelled.is_empty() {
        0.0
    } else {
        labelled.iter().map(|l| l.confidence).sum::<f64>() / labelled.len() as f64
    };

    Some(ClassifyResult {
        class: overall,
        reason: raw.reason,
        candidates: labelled,
        confidence: mean_conf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::edit_diff::Hunk;

    fn hunk(t: &str, p: &str, k: &str) -> Hunk {
        Hunk {
            transcript_window: t.into(),
            polish_window: p.into(),
            kept_window: k.into(),
        }
    }

    #[test]
    fn parse_stt_error_label_is_paired_with_hunk() {
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"STT_ERROR","reason":"n8n is jargon",
                       "labels":[{"hunk_index":0,"class":"STT_ERROR","confidence":0.9}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::SttError);
        assert_eq!(r.candidates.len(), 1);
        assert_eq!(r.candidates[0].correct_form(), "n8n");
        assert_eq!(r.candidates[0].class, EditClass::SttError);
        assert!(r.class.is_learnable());
    }

    #[test]
    fn parse_polish_error_label() {
        let hunks = vec![hunk("please", "kindly", "please")];
        let json = r#"{"overall":"POLISH_ERROR","reason":"AI swap",
                       "labels":[{"hunk_index":0,"class":"POLISH_ERROR","confidence":0.85}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::PolishError);
        assert!(r.class.is_learnable());
    }

    #[test]
    fn parse_rephrase_no_promotion() {
        let hunks = vec![hunk("good", "good", "great")];
        let json = r#"{"overall":"USER_REPHRASE","reason":"style","labels":[
                        {"hunk_index":0,"class":"USER_REPHRASE","confidence":0.6}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::UserRephrase);
        assert!(!r.class.is_learnable());
    }

    #[test]
    fn parse_rewrite() {
        let hunks = vec![hunk("", "", "[link](mailto:x@y)")];
        let json = r#"{"overall":"USER_REWRITE","reason":"prefix added","labels":[
                        {"hunk_index":0,"class":"USER_REWRITE","confidence":0.95}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::UserRewrite);
        assert!(!r.class.is_learnable());
    }

    #[test]
    fn parse_missing_label_defaults_to_rephrase() {
        // LLM returned only one label but there are two hunks — the missing
        // hunk gets safe-default USER_REPHRASE so we don't promote it.
        let hunks = vec![
            hunk("written", "written", "n8n"),
            hunk("good", "good", "great"),
        ];
        let json = r#"{"overall":"STT_ERROR","reason":"only first labelled",
                       "labels":[{"hunk_index":0,"class":"STT_ERROR","confidence":0.9}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.candidates.len(), 2);
        assert_eq!(r.candidates[1].class, EditClass::UserRephrase);
    }

    #[test]
    fn parse_overall_inferred_from_labels_when_missing() {
        // No `overall` field — must derive from labels (priority: STT_ERROR > others).
        let hunks = vec![hunk("a", "a", "b"), hunk("c", "c", "d")];
        let json = r#"{"reason":"","labels":[
                        {"hunk_index":0,"class":"USER_REPHRASE","confidence":0.5},
                        {"hunk_index":1,"class":"STT_ERROR","confidence":0.9}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::SttError);
    }

    #[test]
    fn parse_rejects_unknown_overall_with_no_labels() {
        let hunks = vec![hunk("a", "a", "b")];
        let json = r#"{"overall":"MAYBE","reason":"","labels":[]}"#;
        assert!(parse_label_response(json, &hunks).is_none());
    }

    #[test]
    fn parse_accepts_lowercase_class() {
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"stt_error","labels":[
                        {"hunk_index":0,"class":"stt_error","confidence":0.7}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.class, EditClass::SttError);
    }

    #[test]
    fn parse_confidence_clamped() {
        let hunks = vec![hunk("a", "a", "b")];
        let json = r#"{"overall":"STT_ERROR","labels":[
                        {"hunk_index":0,"class":"STT_ERROR","confidence":2.5}]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert!(r.candidates[0].confidence <= 1.0);
        assert!(r.confidence <= 1.0);
    }

    // ── extracted_term tests ──────────────────────────────────────────────────

    #[test]
    fn composite_hunk_extracts_just_the_name() {
        // The exact production case: user replaced "Anis at the rate Gmail dot
        // com" with "[anish@gmail.com](mailto:anish@gmail.com)".  Only "anish"
        // is the learnable STT correction; the link wrapping is REWRITE shape.
        let hunks = vec![hunk(
            "Anis at the rate Gmail dot com",
            "Anis at the rate Gmail dot com",
            "[anish@gmail.com](mailto:anish@gmail.com)",
        )];
        let json = r#"{"overall":"STT_ERROR","reason":"Anis was misheard for anish",
                       "labels":[{
                         "hunk_index":0,"class":"STT_ERROR","confidence":0.9,
                         "extracted_term":{"transcript_form":"Anis","correct_form":"anish"}
                       }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.candidates.len(), 1);
        let cand = &r.candidates[0];
        // The candidate's getters now return the extracted token, NOT the
        // whole hunk.  This is what Stage-4 promotion sees.
        assert_eq!(cand.transcript_form(), "Anis");
        assert_eq!(cand.correct_form(), "anish");
        // The original hunk text is preserved for inspection.
        assert_eq!(
            cand.hunk.kept_window,
            "[anish@gmail.com](mailto:anish@gmail.com)"
        );
        assert_eq!(cand.extracted_term.as_ref().unwrap().correct_form, "anish");
    }

    #[test]
    fn extracted_term_rejected_if_not_in_kept_window() {
        // LLM hallucinated a correct_form that doesn't appear in kept_window.
        // The parser must drop the extraction (set to None) — promotion
        // can still fall back to kept_window via the getters.
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.9,
                        "extracted_term":{"transcript_form":"written","correct_form":"FAKE"}
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert!(
            r.candidates[0].extracted_term.is_none(),
            "FAKE is not in kept_window — extraction must be rejected"
        );
        // Getters fall back to kept_window.
        assert_eq!(r.candidates[0].correct_form(), "n8n");
    }

    #[test]
    fn extracted_term_rejected_if_not_in_polish_window() {
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.9,
                        "extracted_term":{"transcript_form":"FAKE","correct_form":"n8n"}
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert!(r.candidates[0].extracted_term.is_none());
    }

    #[test]
    fn single_word_hunk_extracted_term_equals_hunk() {
        // For atomic hunks the extracted_term is just the hunk itself.
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.9,
                        "extracted_term":{"transcript_form":"written","correct_form":"n8n"}
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert_eq!(r.candidates[0].correct_form(), "n8n");
        assert_eq!(r.candidates[0].transcript_form(), "written");
    }

    #[test]
    fn missing_extracted_term_falls_back_to_hunk() {
        let hunks = vec![hunk("written", "written", "n8n")];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.9
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert!(r.candidates[0].extracted_term.is_none());
        assert_eq!(r.candidates[0].correct_form(), "n8n");
    }

    #[test]
    fn whole_word_substring_does_not_match_partial() {
        // "ai" must NOT match inside "anish"; only whole-word matches count.
        let hunks = vec![hunk("anish", "anish", "anish.org")];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.9,
                        "extracted_term":{"transcript_form":"ai","correct_form":"ai"}
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        assert!(
            r.candidates[0].extracted_term.is_none(),
            "'ai' is a substring of 'anish' but not a whole word — must reject"
        );
    }

    #[test]
    fn extracted_term_within_markdown_link_is_accepted() {
        // The exact case: "anish" appears as a whole word inside the link.
        let hunks = vec![hunk(
            "Anis",
            "Anis",
            "[anish@gmail.com](mailto:anish@gmail.com)",
        )];
        let json = r#"{"overall":"STT_ERROR","labels":[{
                        "hunk_index":0,"class":"STT_ERROR","confidence":0.85,
                        "extracted_term":{"transcript_form":"Anis","correct_form":"anish"}
                      }]}"#;
        let r = parse_label_response(json, &hunks).unwrap();
        let et = r.candidates[0]
            .extracted_term
            .as_ref()
            .expect("'anish' is whole-word inside the link — must be accepted");
        assert_eq!(et.correct_form, "anish");
    }
}
