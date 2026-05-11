//! Deepgram Live Streaming client (P5 speed optimisation).
//!
//! Opens a WebSocket to `wss://api.deepgram.com/v1/listen` at the START of a
//! recording session and forwards raw audio chunks as they arrive from the
//! microphone.  By the time the user releases Caps Lock, Deepgram has already
//! processed most of the audio in real-time, so its final transcript arrives in
//! ~100–200 ms instead of the ~1 200–2 000 ms of the batch HTTP API.

use std::sync::mpsc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use said_core::deepgram::{BiasPackage, TranscriptMeta, build_ws_url};
use said_recorder::{ChunkReceiver, SAMPLE_RATE, resample_to_16k};
use serde_json::Value;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tracing::{debug, info, warn};

/// Confidence threshold — words below this get [word?XX%] markers for the LLM.
const LOW_CONFIDENCE_THRESHOLD: f64 = 0.85;

#[derive(Debug, Clone)]
pub struct StreamingTranscript {
    pub transcript: String,
    pub meta: TranscriptMeta,
}

type DgWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_ws(deepgram_key: &str, bias: &BiasPackage) -> Option<DgWs> {
    if deepgram_key.is_empty() {
        return None;
    }
    let url_str = build_ws_url("wss://api.deepgram.com/v1/listen", bias, SAMPLE_RATE);

    let mut req = match url_str.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            warn!("[dg_stream] bad WS URL: {e}");
            return None;
        }
    };
    let auth_value = match format!("Token {deepgram_key}").parse() {
        Ok(v) => v,
        Err(e) => {
            warn!("[dg_stream] invalid auth header value: {e}");
            return None;
        }
    };
    req.headers_mut().insert("Authorization", auth_value);

    let start = tokio::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), connect_async(req)).await;
    let ms = start.elapsed().as_millis();

    match result {
        Err(_) => {
            warn!("[dg_stream] WS connect timed out");
            None
        }
        Ok(Err(e)) => {
            warn!("[dg_stream] WS connect failed: {e}");
            None
        }
        Ok(Ok((ws, _))) => {
            info!(
                "[dg_stream] ✓ WS connected in {ms}ms (lang={} keyterms={} replacements={})",
                bias.stt_mode,
                bias.keyterms.len(),
                bias.replacements.len(),
            );
            Some(ws)
        }
    }
}

/// Stream audio to Deepgram and return the final transcript.
///
/// `prewarmed`: if Some and params match, uses the pre-established connection
/// (eliminates TLS handshake from hot path). Falls back to fresh connect if None
/// or if language/keyterms changed.
pub async fn stream_to_deepgram(
    chunk_recv: ChunkReceiver,
    deepgram_key: &str,
    bias: &BiasPackage,
    pre_embed: Option<(&str, &str)>,
) -> Option<StreamingTranscript> {
    if deepgram_key.is_empty() {
        warn!("[dg_stream] no Deepgram API key — WS streaming disabled");
        return None;
    }

    let ws = connect_ws(deepgram_key, bias).await?;

    let (mut ws_tx, mut ws_rx) = ws.split();

    // ── Bridge: std::sync::mpsc (cpal audio thread) → tokio channel ──────────
    let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let native_rate = chunk_recv.native_rate;
    let sync_rx: mpsc::Receiver<Vec<f32>> = chunk_recv.rx;

    std::thread::spawn(move || {
        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            let pcm_bytes: Vec<u8> = resampled
                .iter()
                .flat_map(|&s| {
                    let i16_val = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                    i16_val.to_le_bytes()
                })
                .collect();
            if async_tx.blocking_send(pcm_bytes).is_err() {
                break;
            }
        }
        // sync_rx closed = recording stopped; async_tx drops here
    });

    // ── Main loop: send PCM chunks + receive Deepgram messages ───────────────
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut total_word_count = 0usize;
    let mut total_low_confidence = 0usize;
    let mut total_confidence = 0.0_f64;
    let mut languages_seen: Vec<String> = vec![];
    let mut chunks_sent = 0usize;
    // Track whether speech_final arrived during streaming so the drain loop
    // can start its 500ms timer immediately instead of waiting the full 2500ms.
    let mut got_speech_final_during_stream = false;

    // KeepAlive ticker: send KeepAlive if no audio for 8 s
    let mut keepalive_interval = tokio::time::interval(Duration::from_secs(8));
    keepalive_interval.tick().await; // consume the immediate first tick

    let exit_reason;
    loop {
        tokio::select! {
            chunk = async_rx.recv() => {
                match chunk {
                    Some(pcm) => {
                        chunks_sent += 1;
                        if ws_tx.send(Message::Binary(pcm)).await.is_err() {
                            warn!("[dg_stream] WS send error after {chunks_sent} chunks");
                            exit_reason = "send-error";
                            break;
                        }
                        keepalive_interval.reset();
                    }
                    None => {
                        // Audio channel closed → recording stopped
                        let close = r#"{"type":"CloseStream"}"#;
                        if let Err(e) = ws_tx.send(Message::Text(close.into())).await {
                            warn!("[dg_stream] CloseStream send failed: {e}");
                        }
                        // ── Speculative pre-embed ──────────────────────────────
                        // Fire before the 500ms drain window so Gemini embedding
                        // is cached by the time /v1/voice/polish is called.
                        if let Some((url, secret)) = pre_embed {
                            let plain = plain_for_embed(&transcript_parts);
                            if !plain.is_empty() {
                                let url    = url.to_string();
                                let secret = secret.to_string();
                                debug!("[dg_stream] firing pre-embed ({} chars)", plain.len());
                                tokio::spawn(async move {
                                    let client = reqwest::Client::new();
                                    let body   = serde_json::json!({"text": plain});
                                    if let Err(e) = client
                                        .post(&url)
                                        .header("Authorization", format!("Bearer {secret}"))
                                        .json(&body)
                                        .timeout(std::time::Duration::from_secs(5))
                                        .send()
                                        .await
                                    {
                                        debug!("[dg_stream] pre-embed request failed: {e}");
                                    }
                                });
                            }
                        }
                        exit_reason = "audio-ended";
                        break;
                    }
                }
            }

            _ = keepalive_interval.tick() => {
                debug!("[dg_stream] sending KeepAlive (no audio for 8 s)");
                let ka = r#"{"type":"KeepAlive"}"#;
                if ws_tx.send(Message::Text(ka.into())).await.is_err() {
                    warn!("[dg_stream] KeepAlive send failed");
                    exit_reason = "send-error";
                    break;
                }
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            let msg_type = v["type"].as_str().unwrap_or("?");
                            if msg_type == "Results" {
                                let is_f = v["is_final"].as_bool().unwrap_or(false);
                                let sp_f = v["speech_final"].as_bool().unwrap_or(false);
                                if is_f {
                                    if let Some(chunk) = extract_result_chunk(&v) {
                                        let enriched = chunk.enriched.clone();
                                        info!("[dg_stream] segment: {enriched:?} (speech_final={sp_f})");
                                        transcript_parts.push(enriched);
                                        total_word_count += chunk.word_count;
                                        total_low_confidence += chunk.low_confidence_count;
                                        total_confidence += chunk.confidence_sum;
                                        for language in chunk.languages {
                                            if !languages_seen.iter().any(|seen| seen == &language) {
                                                languages_seen.push(language);
                                            }
                                        }
                                    }
                                }
                                if sp_f {
                                    got_speech_final_during_stream = true;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        warn!("[dg_stream] server closed WS during streaming: {:?}", frame);
                        exit_reason = "server-closed";
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("[dg_stream] WS error during streaming: {e}");
                        exit_reason = "ws-error";
                        break;
                    }
                    None => { exit_reason = "stream-ended"; break; }
                    _ => {}
                }
            }
        }
    }

    let parts_from_streaming = transcript_parts.len();

    // ── Drain remaining messages after CloseStream ────────────────────────────
    // Keep ws_tx alive here: dropping SplitSink before the drain could trigger
    // a WS close handshake, causing ws_rx.next() to return None immediately
    // and losing Deepgram's final is_final Results.
    let _keep_tx_alive = ws_tx;

    // Give Deepgram time to flush remaining utterances after CloseStream.
    //
    // Fast path: if speech_final already arrived during streaming (common for short
    // clips), pre-seed last_speech_final so the drain exits after just 500ms instead
    // of waiting the full 2500ms fallback budget.
    //
    // Fallback: 2500ms flat ceiling for the rare case where Deepgram sends nothing
    // during drain (e.g. very short clip fully processed before CloseStream).
    // Previously this was `chunks_sent * 12` which scaled to 10s+ for long recordings
    // — a flat ceiling is sufficient since the speech_final+500ms logic handles timing.
    let drain_ms = 2500_u64;
    let drain_start = tokio::time::Instant::now();
    let drain_deadline = drain_start + Duration::from_millis(drain_ms);

    let mut last_speech_final: Option<tokio::time::Instant> = if got_speech_final_during_stream {
        Some(tokio::time::Instant::now())
    } else {
        None
    };

    loop {
        let remaining = drain_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let effective_timeout = if let Some(sf_at) = last_speech_final {
            let since_sf = tokio::time::Instant::now().saturating_duration_since(sf_at);
            let sf_remaining = Duration::from_millis(500).saturating_sub(since_sf);
            if sf_remaining.is_zero() {
                break;
            }
            remaining.min(sf_remaining)
        } else {
            remaining
        };

        match tokio::time::timeout(effective_timeout, ws_rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    let msg_type = v["type"].as_str().unwrap_or("?");
                    if msg_type == "Results" {
                        let is_f = v["is_final"].as_bool().unwrap_or(false);
                        let sp_f = v["speech_final"].as_bool().unwrap_or(false);
                        if is_f {
                            if let Some(chunk) = extract_result_chunk(&v) {
                                transcript_parts.push(chunk.enriched);
                                total_word_count += chunk.word_count;
                                total_low_confidence += chunk.low_confidence_count;
                                total_confidence += chunk.confidence_sum;
                                for language in chunk.languages {
                                    if !languages_seen.iter().any(|seen| seen == &language) {
                                        languages_seen.push(language);
                                    }
                                }
                            }
                        }
                        if sp_f {
                            last_speech_final = Some(tokio::time::Instant::now());
                        }
                    } else if msg_type == "UtteranceEnd" {
                        last_speech_final = Some(tokio::time::Instant::now());
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(e))) => {
                warn!("[dg_stream] drain error: {e}");
                break;
            }
            Ok(Some(Ok(_))) => {}
            Err(_) => break,
        }
    }

    // ws_tx drops here (after drain)
    drop(_keep_tx_alive);

    let full = transcript_parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let drain_ms = drain_start.elapsed().as_millis();
    if full.is_empty() {
        warn!("[dg_stream] no transcript — chunks={chunks_sent} drain={drain_ms}ms");
        None
    } else {
        let mean_word_confidence = if total_word_count == 0 {
            0.0
        } else {
            total_confidence / total_word_count as f64
        };
        info!(
            "[dg_stream] ✓ transcript ready — drain={}ms chunks={} parts={} : {full:?}",
            drain_ms,
            chunks_sent,
            transcript_parts.len()
        );
        Some(StreamingTranscript {
            transcript: full.clone(),
            meta: TranscriptMeta {
                enriched_transcript: full,
                confidence: mean_word_confidence,
                mean_word_confidence,
                low_confidence_count: total_low_confidence,
                word_count: total_word_count,
                languages: languages_seen,
                stt_mode: bias.stt_mode.clone(),
            },
        })
    }
}

/// Build enriched text from Deepgram's `words` array inside a Results message.
/// Words with confidence < threshold are marked `[word?XX%]` so the LLM knows
/// which words to scrutinize for context-based correction.
///
/// Falls back to joining plain `punctuated_word`/`word` fields if parsing fails.
struct ResultChunk {
    enriched: String,
    word_count: usize,
    low_confidence_count: usize,
    confidence_sum: f64,
    languages: Vec<String>,
}

fn extract_result_chunk(v: &Value) -> Option<ResultChunk> {
    let alt = v["channel"]["alternatives"].as_array()?.first()?;
    let words_val = &alt["words"];
    let Some(words) = words_val.as_array() else {
        return None;
    };
    if words.is_empty() {
        return None;
    }

    let mut parts = Vec::with_capacity(words.len());
    let mut languages = Vec::new();
    let mut confidence_sum = 0.0_f64;
    let mut low_confidence_count = 0usize;
    for w in words {
        let word = w["punctuated_word"]
            .as_str()
            .or_else(|| w["word"].as_str())
            .unwrap_or("");
        if word.is_empty() {
            continue;
        }

        let conf = w["confidence"].as_f64().unwrap_or(1.0);
        confidence_sum += conf;
        if let Some(language) = w["language"].as_str() {
            if !languages.iter().any(|seen| seen == language) {
                languages.push(language.to_string());
            }
        }
        if conf < LOW_CONFIDENCE_THRESHOLD {
            parts.push(format!("[{}?{:.0}%]", word, conf * 100.0));
            low_confidence_count += 1;
        } else {
            parts.push(word.to_string());
        }
    }

    if let Some(alt_languages) = alt["languages"].as_array() {
        for language in alt_languages.iter().filter_map(|value| value.as_str()) {
            if !languages.iter().any(|seen| seen == language) {
                languages.push(language.to_string());
            }
        }
    }

    Some(ResultChunk {
        enriched: parts.join(" "),
        word_count: words.len(),
        low_confidence_count,
        confidence_sum,
        languages,
    })
}

/// Strip `[word?XX%]` confidence markers and join transcript parts into plain
/// text suitable for embedding.  The embedding cache key is SHA256 of the plain
/// transcript, so this must match what voice.rs does in `strip_confidence_markers`.
fn plain_for_embed(parts: &[String]) -> String {
    let joined = parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    // Replace each [word?XX%] marker with just the word before the '?'
    let mut result = String::with_capacity(joined.len());
    let mut chars = joined.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            let mut closed = false;
            for ic in chars.by_ref() {
                if ic == ']' {
                    closed = true;
                    break;
                }
                inner.push(ic);
            }
            if closed {
                if let Some(qpos) = inner.rfind('?') {
                    let after = &inner[qpos + 1..];
                    if after.ends_with('%') && after[..after.len() - 1].parse::<f64>().is_ok() {
                        result.push_str(&inner[..qpos]);
                        continue;
                    }
                }
                result.push('[');
                result.push_str(&inner);
                result.push(']');
            } else {
                result.push('[');
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::extract_result_chunk;
    use said_core::deepgram::{BiasPackage, ReplacementRule, build_ws_url};
    use said_recorder::SAMPLE_RATE;
    use serde_json::json;

    #[test]
    fn ws_url_uses_multi_mode_and_replacements() {
        let bias = BiasPackage {
            stt_mode: "multi".into(),
            keyterms: vec!["EMIAC".into()],
            replacements: vec![ReplacementRule {
                find: "n10n".into(),
                replace: Some("n8n".into()),
            }],
        };
        let url = build_ws_url("wss://api.deepgram.com/v1/listen", &bias, SAMPLE_RATE);
        assert!(url.contains("language=multi"));
        assert!(url.contains("endpointing=100"));
        assert!(url.contains("replace=n10n:n8n"));
    }

    #[test]
    fn result_chunk_tracks_languages_and_confidence() {
        let value = json!({
            "channel": {
                "alternatives": [{
                    "languages": ["hi", "en"],
                    "words": [
                        { "word": "EMIAC", "confidence": 0.95, "language": "en" },
                        { "word": "hai", "confidence": 0.61, "language": "hi" }
                    ]
                }]
            }
        });
        let chunk = extract_result_chunk(&value).expect("chunk");
        assert_eq!(chunk.word_count, 2);
        assert_eq!(chunk.low_confidence_count, 1);
        assert!(chunk.languages.contains(&"en".to_string()));
        assert!(chunk.enriched.contains("[hai?61%]"));
    }
}
