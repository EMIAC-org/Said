//! Gemini Embedding 2 client with persistent SQLite cache.
//!
//! Endpoint:
//!   POST https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent
//!   Header: x-goog-api-key: $GEMINI_API_KEY
//!   Body:   { "content": { "parts": [{ "text": "…" }] }, "output_dimensionality": 256 }
//!
//! Gap 4: dimensions reduced from 768 → 256 (Matryoshka truncation preserves quality).
//! Benefits: ~3x faster cosine KNN search, ~3x smaller cache blobs, smaller API response.
//! Migration 011 clears old 768-dim vectors so they are rebuilt at the new size.
//!
//! Caching strategy:
//!   1. SHA-256 the input text → look up `embedding_cache` table
//!   2. On miss: call Gemini, store result, return
//!   3. Retry 3× with exponential backoff on 5xx

use reqwest::Client;
use rusqlite::params;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tracing::{info, trace, warn};

use crate::store::DbPool;

const EMBED_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent";
/// Gap 4: 256-dim Matryoshka truncation (down from 768).
/// ~3x faster KNN, ~3x smaller blobs, same quality at personal corpus size.
const DIMENSIONS: usize = 256;
const RETRY_DELAYS_MS: &[u64] = &[200, 800, 2000];

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: EmbedValues,
}

#[derive(Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}

/// Compute the 768-dimensional embedding for `text`.
///
/// Checks `embedding_cache` first (keyed by SHA-256 of the text).
/// On miss: calls Gemini, stores result in cache, returns embedding.
///
/// `api_key` should be the key from preferences (or env var fallback).
/// Returns `None` if the key is empty or all retries fail (polish still works, just no RAG).
pub async fn embed(client: &Client, pool: &DbPool, text: &str, api_key: &str) -> Option<Vec<f32>> {
    if api_key.is_empty() {
        warn!("[embedder] Gemini API key not set — skipping embedding");
        return None;
    }

    let t0 = Instant::now();
    let hash = sha256_hex(text);

    // ── Cache lookup (spawn_blocking: sync SQLite inside async context) ─────────
    let pool_c = pool.clone();
    let hash_c = hash.clone();
    if let Some(cached) = tokio::task::spawn_blocking(move || read_from_cache(&pool_c, &hash_c))
        .await
        .unwrap_or(None)
    {
        info!(
            "[embedder] GAP-4: cache HIT in {}ms ({DIMENSIONS}d, {} chars)",
            t0.elapsed().as_millis(),
            text.len()
        );
        return Some(cached);
    }

    // ── API call with retry ───────────────────────────────────────────────────
    let body = serde_json::json!({
        "content": { "parts": [{ "text": text }] },
        "output_dimensionality": DIMENSIONS,   // Gap 4: 256 (was 768)
    });

    for (attempt, &delay_ms) in RETRY_DELAYS_MS.iter().enumerate() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        let resp = client
            .post(EMBED_URL)
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        match resp {
            Err(e) => {
                warn!("[embedder] request error (attempt {}): {e}", attempt + 1);
            }
            Ok(r) if r.status().is_server_error() => {
                warn!(
                    "[embedder] server error {} (attempt {})",
                    r.status(),
                    attempt + 1
                );
            }
            Ok(r) if !r.status().is_success() => {
                warn!("[embedder] unexpected status {} — giving up", r.status());
                return None;
            }
            Ok(r) => {
                match r.json::<EmbedResponse>().await {
                    Err(e) => {
                        warn!("[embedder] parse error: {e}");
                        return None;
                    }
                    Ok(embed_resp) => {
                        let values = embed_resp.embedding.values;
                        if values.len() != DIMENSIONS {
                            warn!(
                                "[embedder] unexpected dimension {} (want {DIMENSIONS})",
                                values.len()
                            );
                            return None;
                        }
                        // Persist in cache — fire-and-forget to avoid blocking the return.
                        {
                            let pool_s = pool.clone();
                            let hash_s = hash.clone();
                            let vals = values.clone();
                            tokio::task::spawn_blocking(move || {
                                store_in_cache(&pool_s, &hash_s, &vals)
                            });
                        }
                        info!(
                            "[embedder] GAP-4: API MISS → {DIMENSIONS}d in {}ms ({} chars)",
                            t0.elapsed().as_millis(),
                            text.len()
                        );
                        return Some(values);
                    }
                }
            }
        }
    }

    warn!("[embedder] all retries exhausted for hash {}", &hash[..8]);
    None
}

/// Hot-path lookup only. Never calls Gemini; use this when latency matters and
/// background pre-embed can populate the cache for future recordings.
pub async fn cached(pool: &DbPool, text: &str) -> Option<Vec<f32>> {
    let t0 = Instant::now();
    let hash = sha256_hex(text);
    let pool_c = pool.clone();
    let hash_c = hash.clone();
    let cached = tokio::task::spawn_blocking(move || read_from_cache(&pool_c, &hash_c))
        .await
        .unwrap_or(None);
    if cached.is_some() {
        info!(
            "[embedder] GAP-4: hot cache HIT in {}ms ({DIMENSIONS}d, {} chars)",
            t0.elapsed().as_millis(),
            text.len()
        );
    } else {
        trace!(
            "[embedder] GAP-4: hot cache MISS in {}ms ({} chars)",
            t0.elapsed().as_millis(),
            text.len()
        );
    }
    cached
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Decode a 768-float BLOB from the cache.
fn read_from_cache(pool: &DbPool, hash: &str) -> Option<Vec<f32>> {
    let conn = pool.get().ok()?;
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT embedding FROM embedding_cache WHERE text_hash = ?1",
            params![hash],
            |row| row.get(0),
        )
        .ok()?;

    if blob.len() != DIMENSIONS * 4 {
        return None;
    }

    let floats: Vec<f32> = blob
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();

    Some(floats)
}

/// Encode f32 slice as little-endian bytes and persist.
fn store_in_cache(pool: &DbPool, hash: &str, values: &[f32]) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let blob: Vec<u8> = values.iter().flat_map(|f| f.to_le_bytes()).collect();

    let now_ms = crate::store::now_ms();
    let _ = conn.execute(
        "INSERT OR REPLACE INTO embedding_cache (text_hash, embedding, created_at)
         VALUES (?1, ?2, ?3)",
        params![hash, blob, now_ms],
    );
}

/// Encode `Vec<f32>` → raw bytes for storage in `preference_vectors`.
pub fn floats_to_blob(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode raw bytes → `Vec<f32>` from `preference_vectors`.
pub fn blob_to_floats(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.len() % 4 != 0 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
    )
}
