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
/// Buffered PCM chunks while the Deepgram WebSocket is still connecting.
///
/// The recorder's CoreAudio callback uses a small non-blocking channel so it
/// never stalls the audio thread. If WS connect takes 2-5s and nothing drains
/// that channel, early speech can be dropped before streaming even starts,
/// which then makes the finish path reject the WS transcript and fall back to
/// slow batch STT. This async buffer lets a bridge thread drain CoreAudio
/// immediately while the network handshake is still in progress.
const AUDIO_BRIDGE_BUFFER_CHUNKS: usize = 4096;

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
/// Connects fresh per recording. Audio is bridged into an async buffer before
/// the network handshake completes so slow connects do not drop early speech.
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

    // ── Bridge: std::sync::mpsc (cpal audio thread) → tokio channel ──────────
    let (async_tx, mut async_rx) =
        tokio::sync::mpsc::channel::<Vec<u8>>(AUDIO_BRIDGE_BUFFER_CHUNKS);
    let native_rate = chunk_recv.native_rate;
    let sync_rx: mpsc::Receiver<Vec<f32>> = chunk_recv.rx;

    std::thread::spawn(move || {
        let mut bridged_chunks = 0usize;
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
            bridged_chunks += 1;
        }
        debug!("[dg_stream] audio bridge closed after {bridged_chunks} chunk(s)");
        // sync_rx closed = recording stopped; async_tx drops here
    });

    let ws = connect_ws(deepgram_key, bias).await?;

    let (mut ws_tx, mut ws_rx) = ws.split();

    // ── Main loop: stream while key is held, collect results ──────────────────
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut total_word_count = 0usize;
    let mut total_low_confidence = 0usize;
    let mut total_confidence = 0.0_f64;
    let mut languages_seen: Vec<String> = vec![];
    let mut chunks_sent = 0usize;

    let mut keepalive_interval = tokio::time::interval(Duration::from_secs(8));
    keepalive_interval.tick().await;

    loop {
        tokio::select! {
            chunk = async_rx.recv() => {
                match chunk {
                    Some(pcm) => {
                        chunks_sent += 1;
                        if ws_tx.send(Message::Binary(pcm)).await.is_err() {
                            warn!("[dg_stream] WS send error after {chunks_sent} chunks");
                            break;
                        }
                        keepalive_interval.reset();
                    }
                    None => {
                        let close = r#"{"type":"CloseStream"}"#;
                        if let Err(e) = ws_tx.send(Message::Text(close.into())).await {
                            warn!("[dg_stream] CloseStream send failed: {e}");
                        }
                        if let Some((url, secret)) = pre_embed {
                            let plain = plain_for_embed(&transcript_parts);
                            if !plain.is_empty() {
                                let url    = url.to_string();
                                let secret = secret.to_string();
                                debug!("[dg_stream] firing pre-embed ({} chars)", plain.len());
                                tokio::spawn(async move {
                                    let client = reqwest::Client::new();
                                    let body   = serde_json::json!({"text": plain});
                                    let _ = client
                                        .post(&url)
                                        .header("Authorization", format!("Bearer {secret}"))
                                        .json(&body)
                                        .timeout(std::time::Duration::from_secs(5))
                                        .send()
                                        .await;
                                });
                            }
                        }
                        break;
                    }
                }
            }
            _ = keepalive_interval.tick() => {
                let ka = r#"{"type":"KeepAlive"}"#;
                if ws_tx.send(Message::Text(ka.into())).await.is_err() {
                    break;
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        collect_result(&text, &mut transcript_parts, &mut total_word_count,
                            &mut total_low_confidence, &mut total_confidence, &mut languages_seen);
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }

    // ── Drain: key released, wait for Deepgram to finish ────────────────────
    // Keep ws_tx alive so Deepgram doesn't see a TCP close before it's done.
    let _keep_tx_alive = ws_tx;
    let drain_start = tokio::time::Instant::now();
    let drain_deadline = drain_start + Duration::from_millis(1500);

    loop {
        let remaining = drain_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, ws_rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                collect_result(
                    &text,
                    &mut transcript_parts,
                    &mut total_word_count,
                    &mut total_low_confidence,
                    &mut total_confidence,
                    &mut languages_seen,
                );
            }
            // Deepgram closed the connection → it's done, we're done
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {}
            Err(_) => break, // timeout
        }
    }

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

fn collect_result(
    text: &str,
    parts: &mut Vec<String>,
    word_count: &mut usize,
    low_conf: &mut usize,
    conf_sum: &mut f64,
    langs: &mut Vec<String>,
) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if v["type"].as_str().unwrap_or("") != "Results" {
        return;
    }
    if !v["is_final"].as_bool().unwrap_or(false) {
        return;
    }
    if let Some(chunk) = extract_result_chunk(&v) {
        info!("[dg_stream] segment: {:?}", chunk.enriched);
        parts.push(chunk.enriched);
        *word_count += chunk.word_count;
        *low_conf += chunk.low_confidence_count;
        *conf_sum += chunk.confidence_sum;
        for language in chunk.languages {
            if !langs.iter().any(|seen| seen == &language) {
                langs.push(language);
            }
        }
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
    use super::{AUDIO_BRIDGE_BUFFER_CHUNKS, extract_result_chunk};
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
        assert!(url.contains("endpointing=1000"));
        assert!(url.contains("utterance_end_ms=2000"));
        assert!(url.contains("replace=n10n:n8n"));
    }

    #[test]
    fn audio_bridge_buffer_covers_slow_ws_connects() {
        assert!(
            AUDIO_BRIDGE_BUFFER_CHUNKS >= 4096,
            "audio bridge must absorb several seconds of mic chunks while WS connects"
        );
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
