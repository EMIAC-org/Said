use serde::{Deserialize, Serialize};

pub const DEEPGRAM_MODEL: &str = "nova-3";
pub const MAX_KEYTERMS: usize = 50;
pub const MAX_REPLACEMENTS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplacementRule {
    pub find: String,
    #[serde(default)]
    pub replace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BiasPackage {
    pub stt_mode: String,
    #[serde(default)]
    pub keyterms: Vec<String>,
    #[serde(default)]
    pub replacements: Vec<ReplacementRule>,
}

impl Default for BiasPackage {
    fn default() -> Self {
        Self {
            stt_mode: "hi".to_string(),
            keyterms: vec![],
            replacements: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TranscriptMeta {
    #[serde(default)]
    pub enriched_transcript: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub mean_word_confidence: f64,
    #[serde(default)]
    pub low_confidence_count: usize,
    #[serde(default)]
    pub word_count: usize,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub stt_mode: String,
}

pub fn resolve_stt_mode(language: &str) -> String {
    match language.trim() {
        "" | "auto" | "multi" => "hi".to_string(),
        "hi" => "hi".to_string(),
        "en" => "en".to_string(),
        "en-IN" => "en-IN".to_string(),
        other => other.to_string(),
    }
}

pub fn endpointing_for_mode(_stt_mode: &str) -> u32 {
    1000
}

pub fn build_batch_url(base: &str, bias: &BiasPackage) -> String {
    // Raw mode — no Deepgram post-processing. smart_format OFF, punctuate OFF.
    // All formatting (punctuation, casing, numbers, dates) handled by the LLM.
    // Deepgram's punctuate was dropping words for Hindi (e.g. "meac" vanished
    // from "meac technologies" when punctuate=true).
    let mut url = format!(
        "{base}?model={DEEPGRAM_MODEL}&language={}",
        urlencode(&bias.stt_mode)
    );
    append_bias_params(&mut url, bias);
    url
}

pub fn build_ws_url(base: &str, bias: &BiasPackage, sample_rate: u32) -> String {
    let mut url = format!(
        "{base}?model={DEEPGRAM_MODEL}&language={}&encoding=linear16&sample_rate={sample_rate}&channels=1&interim_results=true&endpointing={}&utterance_end_ms=2000",
        urlencode(&bias.stt_mode),
        endpointing_for_mode(&bias.stt_mode),
    );
    append_bias_params(&mut url, bias);
    url
}

fn append_bias_params(url: &mut String, bias: &BiasPackage) {
    // Keyterm prompting (Nova-3) helps en/multi, but on `hi` it catastrophically
    // over-biases — the model repeats the keyterm and shreds Hindi sentence
    // structure (e.g. "my name is anugra and anugra and anugra...", dropping
    // "गुप्ता है और मैं"). Suppress keyterms on hi. The English rescue requests
    // with stt_mode="multi", so it still carries them where they actually help.
    if bias.stt_mode != "hi" {
        for term in bias
            .keyterms
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .take(MAX_KEYTERMS)
        {
            url.push_str("&keyterm=");
            url.push_str(&urlencode(term));
        }
    }

    for replacement in bias
        .replacements
        .iter()
        .filter(|r| !r.find.trim().is_empty())
        .take(MAX_REPLACEMENTS)
    {
        url.push_str("&replace=");
        url.push_str(&urlencode(replacement.find.trim()));
        if let Some(replace) = replacement.replace.as_deref().map(str::trim) {
            if !replace.is_empty() {
                url.push(':');
                url.push_str(&urlencode(replace));
            }
        }
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_and_multi_resolve_to_hi() {
        assert_eq!(resolve_stt_mode(""), "hi");
        assert_eq!(resolve_stt_mode("auto"), "hi");
        assert_eq!(resolve_stt_mode("multi"), "hi");
        assert_eq!(resolve_stt_mode("hi"), "hi");
    }

    #[test]
    fn batch_url_includes_keyterms_on_multi_and_replacements() {
        let bias = BiasPackage {
            stt_mode: "multi".into(),
            keyterms: vec!["AcmeCorp".into(), "WidgetX".into()],
            replacements: vec![
                ReplacementRule {
                    find: "widget ten".into(),
                    replace: Some("WidgetX".into()),
                },
                ReplacementRule {
                    find: "ack me".into(),
                    replace: Some("AcmeCorp".into()),
                },
            ],
        };
        let url = build_batch_url("https://api.deepgram.com/v1/listen", &bias);
        assert!(url.contains("language=multi"));
        assert!(url.contains("&keyterm=AcmeCorp"));
        assert!(url.contains("&replace=widget%20ten:WidgetX"));
        assert!(url.contains("&replace=ack%20me:AcmeCorp"));
    }

    #[test]
    fn batch_url_suppresses_keyterms_on_hi() {
        // Keyterms over-bias Hindi (Nova-3 hi repeats them and shreds the
        // sentence), so hi requests must carry NO keyterms — deterministic
        // replacements still apply.
        let bias = BiasPackage {
            stt_mode: "hi".into(),
            keyterms: vec!["AcmeCorp".into(), "WidgetX".into()],
            replacements: vec![ReplacementRule {
                find: "ack me".into(),
                replace: Some("AcmeCorp".into()),
            }],
        };
        let url = build_batch_url("https://api.deepgram.com/v1/listen", &bias);
        assert!(url.contains("language=hi"));
        assert!(!url.contains("keyterm="));
        assert!(url.contains("&replace=ack%20me:AcmeCorp"));
    }

    #[test]
    fn ws_url_uses_multi_endpointing() {
        let bias = BiasPackage::default();
        let url = build_ws_url("wss://api.deepgram.com/v1/listen", &bias, 16000);
        assert!(url.contains("endpointing=1000"));
        assert!(url.contains("utterance_end_ms=2000"));
    }

    #[test]
    fn batch_url_raw_mode() {
        let bias = BiasPackage::default();
        let url = build_batch_url("https://api.deepgram.com/v1/listen", &bias);
        assert!(!url.contains("smart_format"), "smart_format must be OFF");
        assert!(
            !url.contains("punctuate"),
            "punctuate must be OFF — LLM handles it"
        );
    }

    #[test]
    fn ws_url_raw_mode() {
        let bias = BiasPackage::default();
        let url = build_ws_url("wss://api.deepgram.com/v1/listen", &bias, 16000);
        assert!(!url.contains("smart_format"), "smart_format must be OFF");
        assert!(
            !url.contains("punctuate"),
            "punctuate must be OFF — LLM handles it"
        );
    }
}
