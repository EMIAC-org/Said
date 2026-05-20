//! Deepgram batch transcription client.
//! POST /v1/listen — returns the top transcript string plus per-word confidence.
//!
//! API docs: https://developers.deepgram.com/reference/listen-file

use reqwest::Client;
use said_core::deepgram::{BiasPackage, TranscriptMeta, build_batch_url};
use serde::Deserialize;
use tracing::{debug, info, warn};

const DEEPGRAM_URL: &str = "https://api.deepgram.com/v1/listen";

/// Confidence threshold below which a word is flagged as uncertain.
/// Deepgram typically gives >0.9 for clear words, 0.4–0.8 for ambiguous ones.
pub const LOW_CONFIDENCE_THRESHOLD: f64 = 0.85;

#[derive(Deserialize)]
struct DGResponse {
    results: DGResults,
}
#[derive(Deserialize)]
struct DGResults {
    channels: Vec<DGChannel>,
}
#[derive(Deserialize)]
struct DGChannel {
    alternatives: Vec<DGAlternative>,
}
#[derive(Deserialize)]
struct DGAlternative {
    transcript: String,
    confidence: f64,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    words: Vec<DGWord>,
}
#[derive(Deserialize)]
struct DGWord {
    word: String,
    confidence: f64,
    #[serde(default)]
    punctuated_word: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

pub struct TranscriptResult {
    /// Plain transcript text (for storage, display, embedding).
    pub transcript: String,
    /// Enriched transcript with low-confidence words marked as [word?XX%].
    /// Sent to the LLM so it knows which words to scrutinize.
    pub enriched_transcript: String,
    /// Overall utterance-level confidence (0.0–1.0).
    pub confidence: f64,
    /// Number of words flagged as low-confidence.
    pub uncertain_count: usize,
    pub mean_word_confidence: f64,
    pub word_count: usize,
    pub languages: Vec<String>,
    pub stt_mode: String,
}

impl TranscriptResult {
    pub fn meta(&self) -> TranscriptMeta {
        TranscriptMeta {
            enriched_transcript: self.enriched_transcript.clone(),
            confidence: self.confidence,
            mean_word_confidence: self.mean_word_confidence,
            low_confidence_count: self.uncertain_count,
            word_count: self.word_count,
            languages: self.languages.clone(),
            stt_mode: self.stt_mode.clone(),
        }
    }
}

/// Send WAV audio bytes to Deepgram and return the top transcript.
/// `language` defaults to `"hi"` (Hindi/Hinglish) if empty.
/// `keyterms` are personal-vocabulary terms that Deepgram should bias toward
/// (Nova-3 keyterm prompting).  Pass an empty slice for no biasing.
pub async fn transcribe(
    client: &Client,
    api_key: &str,
    wav_data: Vec<u8>,
    bias: &BiasPackage,
) -> Result<TranscriptResult, String> {
    let url = build_batch_url(DEEPGRAM_URL, bias);
    if !bias.keyterms.is_empty() || !bias.replacements.is_empty() {
        debug!(
            "[stt] biasing Deepgram with {} keyterm(s), {} replacement(s)",
            bias.keyterms.len().min(said_core::deepgram::MAX_KEYTERMS),
            bias.replacements
                .len()
                .min(said_core::deepgram::MAX_REPLACEMENTS),
        );
    }

    debug!(
        "[stt] sending {} bytes to Deepgram (lang={lang})",
        wav_data.len(),
        lang = bias.stt_mode,
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", "audio/wav")
        .body(wav_data)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Deepgram request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let preview = &body[..body.len().min(300)];
        return Err(format!("Deepgram error {status}: {preview}"));
    }

    let dg: DGResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse Deepgram response: {e}"))?;

    let alt = dg
        .results
        .channels
        .into_iter()
        .next()
        .and_then(|c| c.alternatives.into_iter().next())
        .ok_or_else(|| "Deepgram returned no transcript".to_string())?;

    if alt.transcript.is_empty() {
        warn!("[stt] empty transcript from Deepgram");
        return Err("empty transcript — nothing spoken?".into());
    }

    // Build enriched transcript with confidence markers
    let (enriched, uncertain_count, mean_word_confidence, word_count, word_languages) =
        if alt.words.is_empty() {
            // No word-level data — fall back to plain transcript
            (
                alt.transcript.clone(),
                0,
                alt.confidence,
                alt.transcript.split_whitespace().count(),
                vec![],
            )
        } else {
            enrich_words(&alt.words)
        };

    debug!(
        "[stt] transcript ({:.2}): {}",
        alt.confidence, alt.transcript
    );
    if uncertain_count > 0 {
        info!(
            "[stt] {} uncertain word(s) flagged (threshold < {:.0}%)",
            uncertain_count,
            LOW_CONFIDENCE_THRESHOLD * 100.0
        );
        info!("[stt] enriched: {enriched}");
    }

    Ok(TranscriptResult {
        transcript: alt.transcript,
        enriched_transcript: enriched,
        confidence: alt.confidence,
        uncertain_count,
        mean_word_confidence,
        word_count,
        languages: if alt.languages.is_empty() {
            word_languages
        } else {
            alt.languages
        },
        stt_mode: bias.stt_mode.clone(),
    })
}

/// Like `transcribe`, but auto-detects audio format from magic bytes.
/// Supports WAV, WebM, MP3, OGG, FLAC, and falls back to octet-stream.
pub async fn transcribe_any_format(
    client: &Client,
    api_key: &str,
    audio_data: Vec<u8>,
    bias: &BiasPackage,
) -> Result<TranscriptResult, String> {
    let content_type = detect_audio_content_type(&audio_data);
    let url = build_batch_url(DEEPGRAM_URL, bias);

    debug!(
        "[stt] sending {} bytes to Deepgram as {} (lang={lang})",
        audio_data.len(),
        content_type,
        lang = bias.stt_mode,
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", content_type)
        .body(audio_data)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Deepgram request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let preview = &body[..body.len().min(300)];
        return Err(format!("Deepgram error {status}: {preview}"));
    }

    let dg: DGResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse Deepgram response: {e}"))?;

    let alt = dg
        .results
        .channels
        .into_iter()
        .next()
        .and_then(|c| c.alternatives.into_iter().next())
        .ok_or_else(|| "Deepgram returned no transcript".to_string())?;

    if alt.transcript.is_empty() {
        warn!("[stt] empty transcript from Deepgram");
        return Err("empty transcript — nothing spoken?".into());
    }

    let (enriched, uncertain_count, mean_word_confidence, word_count, word_languages) =
        if alt.words.is_empty() {
            (
                alt.transcript.clone(),
                0,
                alt.confidence,
                alt.transcript.split_whitespace().count(),
                vec![],
            )
        } else {
            enrich_words(&alt.words)
        };

    Ok(TranscriptResult {
        transcript: alt.transcript,
        enriched_transcript: enriched,
        confidence: alt.confidence,
        uncertain_count,
        mean_word_confidence,
        word_count,
        languages: if alt.languages.is_empty() {
            word_languages
        } else {
            alt.languages
        },
        stt_mode: bias.stt_mode.clone(),
    })
}

fn detect_audio_content_type(data: &[u8]) -> &'static str {
    if data.len() >= 4 {
        if &data[..4] == b"RIFF" {
            return "audio/wav";
        }
        if &data[..4] == b"fLaC" {
            return "audio/flac";
        }
        if &data[..4] == b"OggS" {
            return "audio/ogg";
        }
        if data.len() >= 3 && &data[..3] == b"ID3" {
            return "audio/mpeg";
        }
        // WebM/Matroska magic bytes: 0x1A 0x45 0xDF 0xA3
        if data[0] == 0x1A && data[1] == 0x45 && data[2] == 0xDF && data[3] == 0xA3 {
            return "audio/webm";
        }
    }
    "application/octet-stream"
}

/// Build an enriched transcript from word-level data.
/// Words with confidence < threshold are marked as `[word?XX%]`.
fn enrich_words(words: &[DGWord]) -> (String, usize, f64, usize, Vec<String>) {
    let mut parts = Vec::with_capacity(words.len());
    let mut uncertain = 0usize;
    let mut confidence_sum = 0.0_f64;
    let mut languages = Vec::new();

    for w in words {
        let display = w.punctuated_word.as_deref().unwrap_or(&w.word);
        confidence_sum += w.confidence;
        if let Some(language) = &w.language {
            if !languages.iter().any(|seen| seen == language) {
                languages.push(language.clone());
            }
        }

        if w.confidence < LOW_CONFIDENCE_THRESHOLD {
            parts.push(format!("[{}?{:.0}%]", display, w.confidence * 100.0));
            uncertain += 1;
        } else {
            parts.push(display.to_string());
        }
    }

    (
        parts.join(" "),
        uncertain,
        confidence_sum / words.len() as f64,
        words.len(),
        languages,
    )
}

#[cfg(test)]
mod tests {
    use super::DEEPGRAM_URL;
    use said_core::deepgram::{BiasPackage, ReplacementRule, build_batch_url, resolve_stt_mode};

    #[test]
    fn auto_maps_to_hi_for_batch() {
        assert_eq!(resolve_stt_mode("auto"), "hi");
    }

    #[test]
    fn batch_url_contains_replacements() {
        let bias = BiasPackage {
            stt_mode: "hi".into(),
            keyterms: vec!["EMIAC".into()],
            replacements: vec![ReplacementRule {
                find: "n10n".into(),
                replace: Some("n8n".into()),
            }],
        };
        let url = build_batch_url(DEEPGRAM_URL, &bias);
        assert!(url.contains("language=hi"));
        assert!(url.contains("keyterm=EMIAC"));
        assert!(url.contains("replace=n10n:n8n"));
    }
}
