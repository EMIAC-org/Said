//! Safety gate for learned aliases.
//!
//! This module is intentionally conservative. Runtime correction can call the
//! deterministic helpers only. The LLM judge is used only from edit-learning
//! paths and every verdict is cached locally.

use std::{collections::HashSet, time::Duration};

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::store::{DbPool, alias_safety as cache};

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
pub const JUDGE_MODEL: &str = "llama-3.1-8b-instant";
const MIN_CALL_SPACING_MS: u64 = 350;

static JUDGE_RATE_LIMIT: Lazy<Mutex<Option<std::time::Instant>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasSafetyVerdict {
    CommonBlock,
    SafeJargon,
    AmbiguousBlock,
    LlmFailedBlock,
}

impl AliasSafetyVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommonBlock => "common_block",
            Self::SafeJargon => "safe_jargon",
            Self::AmbiguousBlock => "ambiguous_block",
            Self::LlmFailedBlock => "llm_failed_block",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "safe_jargon" => Self::SafeJargon,
            "common_block" => Self::CommonBlock,
            "llm_failed_block" => Self::LlmFailedBlock,
            _ => Self::AmbiguousBlock,
        }
    }

    pub fn allows_learning(self) -> bool {
        self == Self::SafeJargon
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliasSafetyOutcome {
    pub source_norm: String,
    pub verdict: AliasSafetyVerdict,
    pub confidence: f64,
    pub provider: String,
    pub model: String,
    pub reason: String,
}

impl AliasSafetyOutcome {
    pub fn allows_learning(&self) -> bool {
        self.verdict.allows_learning()
    }
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
struct JudgePayload {
    verdict: String,
    confidence: Option<f64>,
    reason: Option<String>,
}

const SYSTEM_PROMPT: &str = "You are a conservative safety classifier for a speech dictation app. \
Your job is ONLY to decide whether a proposed alias source is a common Hindi, Hinglish, or English word/phrase \
that would be dangerous to auto-replace, or rare jargon/proper noun/code-like speech that can be safely learned. \
Return strict JSON only: {\"verdict\":\"common_block|safe_jargon|ambiguous_block\",\"confidence\":0.0-1.0,\"reason\":\"short reason\"}. \
Use common_block for normal words, particles, verbs, pronouns, question words, dictionary words, or ordinary phrases. \
Use safe_jargon only for rare ASR distortions, acronyms, proper nouns, code identifiers, brand names, or technical product names. \
If unsure, use ambiguous_block.";

pub fn normalize_source(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let romanized = if super::script::contains_devanagari(text) {
        super::script::romanize_devanagari(text)
    } else {
        text.to_string()
    };
    let collapsed = collapse_long_vowels(&romanized.to_ascii_lowercase());
    collapsed
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn compact_norm(text: &str) -> String {
    normalize_source(text).split_whitespace().collect()
}

pub fn is_common_alias_source(text: &str) -> bool {
    let norm = normalize_source(text);
    is_common_alias_source_norm(&norm)
}

pub fn is_common_alias_source_norm(norm: &str) -> bool {
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    let compact = tokens.join("");
    if COMMON_PHRASES.contains(norm) || COMMON_WORDS.contains(compact.as_str()) {
        return true;
    }
    tokens.iter().all(|token| is_common_token(token))
}

pub fn is_common_token(token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }
    COMMON_WORDS.contains(token)
}

pub async fn judge_alias_source(
    client: &Client,
    pool: &DbPool,
    user_id: &str,
    groq_key: &str,
    source: &str,
    target: &str,
    context: Option<&str>,
) -> AliasSafetyOutcome {
    let source_norm = normalize_source(source);
    if source_norm.is_empty() {
        return cache_and_return(
            pool,
            user_id,
            source_norm,
            AliasSafetyVerdict::AmbiguousBlock,
            1.0,
            "local",
            "",
            "empty alias source",
        );
    }
    if is_common_alias_source_norm(&source_norm) || crate::tier2::is_in_dictionary(&source_norm) {
        return cache_and_return(
            pool,
            user_id,
            source_norm,
            AliasSafetyVerdict::CommonBlock,
            1.0,
            "local",
            "",
            "source is common/dictionary word",
        );
    }
    if let Some(cached) = cache::get(pool, user_id, &source_norm) {
        return AliasSafetyOutcome {
            source_norm,
            verdict: AliasSafetyVerdict::parse(&cached.verdict),
            confidence: cached.confidence,
            provider: cached.provider,
            model: cached.model,
            reason: cached.reason,
        };
    }
    if groq_key.trim().is_empty() {
        return cache_and_return(
            pool,
            user_id,
            source_norm,
            AliasSafetyVerdict::LlmFailedBlock,
            0.0,
            "local",
            "",
            "Groq key unavailable for alias safety judge",
        );
    }

    let prompt = format!(
        "SOURCE_ALIAS: {source}\nSOURCE_NORMALIZED: {source_norm}\nTARGET_CANONICAL: {target}\nCONTEXT: {}\n\nClassify SOURCE_ALIAS only. Do not invent mappings.",
        context.unwrap_or("- none").trim()
    );
    match call_judge(client, groq_key, &prompt).await {
        Some(payload) => {
            let mut verdict = AliasSafetyVerdict::parse(&payload.verdict);
            if verdict == AliasSafetyVerdict::SafeJargon
                && is_common_alias_source_norm(&source_norm)
            {
                verdict = AliasSafetyVerdict::CommonBlock;
            }
            let confidence = payload.confidence.unwrap_or(0.0).clamp(0.0, 1.0);
            let reason = payload
                .reason
                .unwrap_or_else(|| "LLM alias safety judgment".to_string());
            cache_and_return(
                pool,
                user_id,
                source_norm,
                verdict,
                confidence,
                "groq",
                JUDGE_MODEL,
                &reason,
            )
        }
        None => cache_and_return(
            pool,
            user_id,
            source_norm,
            AliasSafetyVerdict::LlmFailedBlock,
            0.0,
            "groq",
            JUDGE_MODEL,
            "LLM judge failed; blocking auto-learning",
        ),
    }
}

fn cache_and_return(
    pool: &DbPool,
    user_id: &str,
    source_norm: String,
    verdict: AliasSafetyVerdict,
    confidence: f64,
    provider: &str,
    model: &str,
    reason: &str,
) -> AliasSafetyOutcome {
    let _ = cache::upsert(
        pool,
        user_id,
        &source_norm,
        verdict.as_str(),
        confidence,
        provider,
        model,
        reason,
    );
    AliasSafetyOutcome {
        source_norm,
        verdict,
        confidence,
        provider: provider.to_string(),
        model: model.to_string(),
        reason: reason.to_string(),
    }
}

async fn call_judge(client: &Client, groq_key: &str, user_prompt: &str) -> Option<JudgePayload> {
    for attempt in 0..3 {
        wait_for_rate_limit().await;
        let body = json!({
            "model": JUDGE_MODEL,
            "temperature": 0,
            "max_tokens": 120,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_prompt}
            ]
        });
        let resp = client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {groq_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let Ok(resp) = resp else {
            warn!(
                "[alias-safety] judge request failed on attempt {}",
                attempt + 1
            );
            backoff(attempt).await;
            continue;
        };
        if resp.status().is_success() {
            let parsed: ChatResponse = resp.json().await.ok()?;
            let content = parsed.choices.into_iter().next()?.message.content;
            match serde_json::from_str::<JudgePayload>(content.trim()) {
                Ok(payload) => {
                    info!(
                        "[alias-safety] LLM verdict={} confidence={:.2}",
                        payload.verdict,
                        payload.confidence.unwrap_or(0.0)
                    );
                    return Some(payload);
                }
                Err(e) => {
                    warn!("[alias-safety] judge JSON parse failed: {e}");
                    return None;
                }
            }
        }
        let status = resp.status();
        if status.as_u16() == 429 || status.is_server_error() {
            warn!("[alias-safety] judge returned {status}, retrying");
            backoff(attempt).await;
            continue;
        }
        let preview = resp.text().await.unwrap_or_default();
        warn!(
            "[alias-safety] judge returned {status}: {}",
            &preview[..preview.len().min(200)]
        );
        return None;
    }
    None
}

async fn wait_for_rate_limit() {
    let mut last_call = JUDGE_RATE_LIMIT.lock().await;
    if let Some(last) = *last_call {
        let elapsed = last.elapsed();
        let min_spacing = Duration::from_millis(MIN_CALL_SPACING_MS);
        if elapsed < min_spacing {
            tokio::time::sleep(min_spacing - elapsed).await;
        }
    }
    *last_call = Some(std::time::Instant::now());
}

async fn backoff(attempt: usize) {
    let delay_ms = 500_u64.saturating_mul(1_u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}

fn collapse_long_vowels(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if i + 1 < bytes.len() && bytes[i + 1] == bytes[i] && b"aeiou".contains(&bytes[i]) {
            match ch {
                'e' => out.push('i'),
                'o' => out.push('u'),
                _ => out.push(ch),
            }
            i += 2;
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

static COMMON_PHRASES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "kaisa laga",
        "kaisi lagi",
        "kaise lage",
        "aisa laga",
        "aisi lagi",
        "aise lage",
        "ye kaisa",
        "yeh kaisa",
        "ye kaise",
        "yeh kaise",
    ]
    .into_iter()
    .collect()
});

static COMMON_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "base",
        "be",
        "been",
        "but",
        "by",
        "can",
        "color",
        "colour",
        "could",
        "do",
        "does",
        "doing",
        "done",
        "for",
        "from",
        "go",
        "going",
        "good",
        "gray",
        "great",
        "grey",
        "had",
        "has",
        "have",
        "here",
        "how",
        "i",
        "in",
        "is",
        "it",
        "just",
        "like",
        "local",
        "much",
        "nice",
        "of",
        "on",
        "or",
        "post",
        "return",
        "said",
        "say",
        "says",
        "should",
        "super",
        "that",
        "the",
        "there",
        "this",
        "through",
        "time",
        "to",
        "was",
        "what",
        "when",
        "where",
        "which",
        "white",
        "who",
        "will",
        "with",
        "would",
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "hundred",
        "thousand",
        "main",
        "maine",
        "mein",
        "mai",
        "mujhe",
        "mujhse",
        "mujhko",
        "tu",
        "tum",
        "tumne",
        "tumhe",
        "tumse",
        "tumko",
        "aap",
        "aapne",
        "aapse",
        "aapko",
        "hum",
        "humne",
        "humse",
        "humko",
        "humein",
        "yeh",
        "ye",
        "yah",
        "woh",
        "wo",
        "voh",
        "vo",
        "isko",
        "usko",
        "inko",
        "unko",
        "koi",
        "kisko",
        "kiske",
        "kiski",
        "jisko",
        "jiske",
        "jiski",
        "uska",
        "uski",
        "unka",
        "unki",
        "mera",
        "meri",
        "mere",
        "tera",
        "teri",
        "tere",
        "tumhara",
        "tumhari",
        "apna",
        "apni",
        "apne",
        "khud",
        "sab",
        "sabko",
        "sabne",
        "sabse",
        "kya",
        "kaisa",
        "kaisi",
        "kaise",
        "aisa",
        "aisi",
        "aise",
        "kab",
        "kahan",
        "kaha",
        "kyun",
        "kaun",
        "kitna",
        "kitne",
        "kitni",
        "hai",
        "hain",
        "tha",
        "thi",
        "the",
        "hoga",
        "hogi",
        "hoge",
        "hona",
        "hota",
        "hoti",
        "hote",
        "kar",
        "karo",
        "karna",
        "karta",
        "karti",
        "karte",
        "karega",
        "karenge",
        "karegi",
        "de",
        "dena",
        "dedo",
        "deta",
        "deti",
        "dete",
        "dunga",
        "denge",
        "le",
        "lena",
        "leta",
        "leti",
        "lete",
        "lenge",
        "lunga",
        "ja",
        "jaana",
        "jaata",
        "jaati",
        "jaate",
        "jayenge",
        "aana",
        "aata",
        "aati",
        "aate",
        "aayega",
        "aayenge",
        "raha",
        "rahi",
        "rahe",
        "rehna",
        "rehta",
        "rehti",
        "bola",
        "boli",
        "bole",
        "bolo",
        "bolna",
        "bolte",
        "bol",
        "dekho",
        "dekh",
        "dekhna",
        "dekhta",
        "dekhti",
        "dekhte",
        "suno",
        "sunna",
        "sunlo",
        "sunta",
        "sunti",
        "sun",
        "batao",
        "batana",
        "batata",
        "batati",
        "bata",
        "chal",
        "chalo",
        "chalna",
        "mil",
        "milna",
        "rakh",
        "rakhna",
        "ban",
        "baith",
        "uth",
        "khol",
        "rok",
        "laga",
        "lagi",
        "lage",
        "lagta",
        "lagti",
        "lagte",
        "lagana",
        "wala",
        "wali",
        "wale",
        "aur",
        "lekin",
        "par",
        "magar",
        "toh",
        "bhi",
        "hi",
        "na",
        "nahi",
        "nhi",
        "haan",
        "mat",
        "bas",
        "sirf",
        "bilkul",
        "zaroor",
        "shayad",
        "ko",
        "se",
        "pe",
        "ka",
        "ki",
        "ke",
        "agar",
        "jab",
        "tab",
        "phir",
        "warna",
        "kyunki",
        "isliye",
        "abhi",
        "accha",
        "achha",
        "theek",
        "sahi",
        "galat",
        "pehle",
        "baad",
        "bahut",
        "thoda",
        "zyada",
        "kam",
        "bada",
        "badi",
        "chhota",
        "chhoti",
        "naya",
        "purana",
        "dono",
        "dusra",
        "dusri",
        "teesra",
        "yaar",
        "bhai",
        "zara",
        "please",
        "din",
        "raat",
        "ghar",
        "log",
        "banda",
        "cheez",
        "jagah",
        "waqt",
        "baar",
        "baat",
        "kaam",
        "aaj",
        "kal",
        "aajkal",
        "ajkal",
        "parso",
        "subah",
        "shaam",
        "dopahar",
        "hamesha",
        "kabhi",
        "tarah",
        "saath",
        "andar",
        "bahar",
        "upar",
        "neeche",
        "peeche",
        "saamne",
        "course",
        "corps",
        "meaning",
        "think",
        "return",
        "accounts",
        "account",
        "google",
        "prayer",
        "house",
        "table",
        "next",
        "rest",
        "capital",
        "cancer",
        "supply",
        "sent",
        "tell",
        "oath",
        "choices",
        "choice",
        "army",
        "division",
        "regular",
        "submitted",
        "worker",
        "company",
    ]
    .into_iter()
    .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn romanized_hindi_common_words_are_blocked() {
        for word in ["कैसा", "kaisa", "kaisi", "kaise", "laga", "main", "mein"] {
            assert!(is_common_alias_source(word), "{word} should be blocked");
        }
    }

    #[test]
    fn common_phrases_are_blocked() {
        for phrase in ["ye kaisa laga", "यह कैसा लगा", "kaisi lagi"] {
            assert!(is_common_alias_source(phrase), "{phrase} should be blocked");
        }
    }

    #[test]
    fn protected_jargon_shapes_are_not_locally_blocked() {
        for word in [
            "Macobs",
            "EMIAC",
            "Kubernetes",
            "GraphQL",
            "n8n",
            "Supabase",
        ] {
            assert!(
                !is_common_alias_source(word),
                "{word} should not be hard-blocked locally"
            );
        }
    }
}
