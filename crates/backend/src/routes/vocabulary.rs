//! Vocabulary HTTP routes.
//!
//! GET    /v1/vocabulary/terms      — light: top-N term strings (STT-bias hot path)
//! GET    /v1/vocabulary            — full: rows with weight/source/use_count
//! POST   /v1/vocabulary            — manual add (source = 'manual')
//! DELETE /v1/vocabulary/:term      — hard remove a single term
//! POST   /v1/vocabulary/:term/star — toggle starred status (immune to demotion)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    AppState,
    embedder::gemini,
    llm::meaning,
    store::{
        now_ms, pending_promotions, prefs::get_prefs, stt_replacements, vocab_embeddings,
        vocab_fts, vocabulary,
    },
};

pub fn spawn_prompt_artifact_repair(state: AppState) {
    tokio::spawn(async move {
        let user_id = state.default_user_id.to_string();
        let total = vocabulary::count(&state.pool, &user_id).max(0) as usize;
        if total == 0 {
            return;
        }

        let pool_terms = state.pool.clone();
        let user_terms = user_id.clone();
        let terms = tokio::task::spawn_blocking(move || {
            vocabulary::top_terms(&pool_terms, &user_terms, total)
        })
        .await
        .unwrap_or_default();
        if terms.is_empty() {
            return;
        }

        let prefs = get_prefs(&state.pool, &state.default_user_id);
        let gemini_key = prefs
            .as_ref()
            .and_then(|p| p.gemini_api_key.clone())
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        let groq_key = prefs
            .as_ref()
            .and_then(|p| p.groq_api_key.clone())
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();
        let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();

        let mut meaning_repaired = 0;
        let mut example_rings_repaired = 0;
        let mut centroids_repaired = 0;

        for term in terms {
            if state.watchdog.is_shedding() {
                tracing::info!("[bg] artifact-repair paused — watchdog shedding load");
                break;
            }
            vocab_fts::upsert(
                &state.pool,
                &state.default_user_id,
                &term.term,
                term.example_context.as_deref(),
            );

            let Some(context) = term
                .example_context
                .clone()
                .filter(|c| !c.trim().is_empty())
            else {
                continue;
            };

            if !vocab_embeddings::has_example_ring(&state.pool, &user_id, &term.term) {
                if !gemini_key.is_empty() {
                    let embed_text = format!("{}. {}", term.term, context);
                    if let Some(embedding) =
                        gemini::embed(&state.http_client, &state.pool, &embed_text, &gemini_key)
                            .await
                    {
                        vocab_embeddings::record_example_and_recentre(
                            &state.pool,
                            &user_id,
                            &term.term,
                            &embedding,
                            &embed_text,
                        );
                        example_rings_repaired += 1;
                    }
                }
            } else if !vocab_embeddings::has_centroid(&state.pool, &user_id, &term.term)
                && vocab_embeddings::rebuild_centroid_from_examples(
                    &state.pool,
                    &user_id,
                    &term.term,
                )
            {
                centroids_repaired += 1;
            }

            if term
                .meaning
                .as_deref()
                .map(|m| m.trim().is_empty())
                .unwrap_or(true)
            {
                if let Some(new_meaning) = meaning::generate_initial(
                    &state.http_client,
                    &groq_key,
                    &openai_key,
                    None,
                    &term.term,
                    &context,
                )
                .await
                {
                    if vocabulary::update_meaning(&state.pool, &user_id, &term.term, &new_meaning) {
                        meaning_repaired += 1;
                    }
                }
            }
        }

        if meaning_repaired > 0 || example_rings_repaired > 0 || centroids_repaired > 0 {
            info!(
                "[vocab-repair] prompt artifacts repaired meanings={} example_rings={} centroids={}",
                meaning_repaired, example_rings_repaired, centroids_repaired,
            );
        }
    });
}

// ── GET /v1/vocabulary/terms (hot path) ──────────────────────────────────────

#[derive(Serialize)]
pub struct TermsResponse {
    pub terms: Vec<String>,
}

#[derive(Deserialize, Default)]
pub struct TermsQuery {
    /// Optional output_language filter.  When supplied, returns only terms
    /// learned in this language (plus legacy language=NULL terms).  Without
    /// this, returns the language-agnostic top-N (legacy behaviour).
    pub language: Option<String>,
}

pub async fn list_terms(
    State(state): State<AppState>,
    Query(q): Query<TermsQuery>,
) -> Json<TermsResponse> {
    // Resolve effective language: explicit query parameter wins, else fall
    // back to the user's stored output_language preference.  This means the
    // desktop hot path automatically gets language-bucketed keyterms with
    // no client-side change required.
    let lang = q.language.filter(|s| !s.trim().is_empty()).or_else(|| {
        get_prefs(&state.pool, &state.default_user_id)
            .map(|p| p.output_language)
            .filter(|s| !s.trim().is_empty())
    });

    let terms = match lang.as_deref() {
        Some(lang) => vocabulary::top_term_strings_for_language(
            &state.pool,
            &state.default_user_id,
            lang,
            100,
        ),
        None => vocabulary::top_term_strings(&state.pool, &state.default_user_id, 100),
    };
    Json(TermsResponse { terms })
}

// ── GET /v1/vocabulary (management UI) ───────────────────────────────────────

#[derive(Serialize)]
pub struct VocabListResponse {
    pub terms: Vec<vocabulary::VocabTerm>,
    pub total: i64,
}

pub async fn list(State(state): State<AppState>) -> Json<VocabListResponse> {
    let terms = vocabulary::top_terms(&state.pool, &state.default_user_id, 500);
    let total = vocabulary::count(&state.pool, &state.default_user_id);
    Json(VocabListResponse { terms, total })
}

// ── POST /v1/vocabulary (manual add) ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateBody {
    pub term: String,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> (StatusCode, Json<serde_json::Value>) {
    let trimmed = body.term.trim();
    if trimmed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "term cannot be empty" })),
        );
    }
    if trimmed.chars().count() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "term too long (max 64 chars)" })),
        );
    }
    // Manual entries land at weight=1.5 (slightly above auto-default 1.0) so
    // user-curated terms outrank a fresh auto-promoted one.
    let ok = vocabulary::upsert(&state.pool, &state.default_user_id, trimmed, 1.5, "manual");
    if ok {
        // Sync FTS index for BM25 retrieval. Manual adds usually have no
        // example_context yet — the FTS row is keyed on the term alone
        // until a future sighting fills in context.
        vocab_fts::upsert(&state.pool, &state.default_user_id, trimmed, None);
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
        info!("[vocab] manual add: {trimmed:?}");
        (
            StatusCode::CREATED,
            Json(serde_json::json!({ "term": trimmed })),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "insert failed" })),
        )
    }
}

// ── DELETE /v1/vocabulary/all — wipe all learning data ───────────────────────

pub async fn delete_all(State(state): State<AppState>) -> StatusCode {
    let uid = state.default_user_id.as_str();
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[vocab] delete_all pool error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let v = conn
        .execute("DELETE FROM vocabulary WHERE user_id = ?1", params![uid])
        .unwrap_or(0);
    let s = conn
        .execute(
            "DELETE FROM stt_replacements WHERE user_id = ?1",
            params![uid],
        )
        .unwrap_or(0);
    let c = conn
        .execute("DELETE FROM corrections WHERE user_id = ?1", params![uid])
        .unwrap_or(0);
    let e = conn
        .execute(
            "DELETE FROM vocab_embeddings WHERE user_id = ?1",
            params![uid],
        )
        .unwrap_or(0);
    let f = conn
        .execute("DELETE FROM vocab_fts WHERE user_id = ?1", params![uid])
        .unwrap_or(0);
    let ex = conn
        .execute(
            "DELETE FROM vocab_embedding_examples WHERE user_id = ?1",
            params![uid],
        )
        .unwrap_or(0);
    drop(conn);
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    info!(
        "[vocab] delete_all: vocab={v} stt={s} corrections={c} embeddings={e} fts={f} examples={ex}",
    );
    StatusCode::NO_CONTENT
}

// ── DELETE /v1/vocabulary/:term ──────────────────────────────────────────────

pub async fn delete(State(state): State<AppState>, Path(term): Path<String>) -> StatusCode {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[vocab] delete pool error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let n = conn
        .execute(
            "DELETE FROM vocabulary WHERE user_id = ?1 AND term = ?2",
            params![state.default_user_id.as_str(), trimmed],
        )
        .unwrap_or(0);
    drop(conn);

    // Cascade-clean every per-term side table. None of these have FK cascades
    // back to `vocabulary`, so each must be cleared explicitly. Skipping any
    // one of them leaves a different ghost behaviour:
    //   • vocab_embeddings    — stale centroid surfaces in dense retrieval
    //   • vocab_fts           — stale BM25 hit surfaces in lexical gate
    //   • vocab_embedding_examples — zombie ring resurfaces if term re-added
    //   • stt_replacements    — pre-polish layer keeps rewriting → canonical
    //   • pending_promotions  — queued sightings can resurrect deleted terms
    vocab_embeddings::delete(&state.pool, &state.default_user_id, trimmed);
    vocab_fts::delete(&state.pool, &state.default_user_id, trimmed);
    let stt_n =
        stt_replacements::delete_by_correct_form(&state.pool, &state.default_user_id, trimmed);
    let pending_n =
        pending_promotions::delete_all_for_term(&state.pool, &state.default_user_id, trimmed);
    info!("[vocab] delete term={trimmed:?} vocab_rows={n} stt_aliases={stt_n} pending={pending_n}",);
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    if n > 0 {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── PATCH /v1/vocabulary/:term ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PatchBody {
    pub meaning: Option<String>,
    pub term_type: Option<String>,
    pub example_context: Option<String>,
}

pub async fn patch(
    State(state): State<AppState>,
    Path(term): Path<String>,
    Json(body): Json<PatchBody>,
) -> StatusCode {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    let uid = state.default_user_id.as_str();
    let mut updated = false;
    if let Some(ref meaning) = body.meaning {
        updated |= vocabulary::update_meaning(&state.pool, uid, trimmed, meaning);
    }
    if let Some(ref tt) = body.term_type {
        updated |= vocabulary::update_term_type(&state.pool, uid, trimmed, tt);
    }
    if let Some(ref ctx) = body.example_context {
        updated |= vocabulary::update_example_context(&state.pool, uid, trimmed, ctx);
    }
    if updated {
        info!("[vocab] patched term={trimmed:?}");
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── POST /v1/vocabulary/:term/star ───────────────────────────────────────────

#[derive(Serialize)]
pub struct StarResponse {
    pub starred: bool,
}

pub async fn toggle_star(
    State(state): State<AppState>,
    Path(term): Path<String>,
) -> (StatusCode, Json<StarResponse>) {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(StarResponse { starred: false }),
        );
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(StarResponse { starred: false }),
            );
        }
    };
    let current_source: Option<String> = conn
        .query_row(
            "SELECT source FROM vocabulary WHERE user_id = ?1 AND term = ?2",
            params![state.default_user_id.as_str(), trimmed],
            |r| r.get(0),
        )
        .ok();

    let Some(source) = current_source else {
        return (StatusCode::NOT_FOUND, Json(StarResponse { starred: false }));
    };

    let (new_source, new_weight, starred) = if source == "starred" {
        // Un-star → revert to manual (preserve weight, just lift the demotion shield).
        ("manual", 1.5_f64, false)
    } else {
        // Star → bump weight high so it sorts to top + becomes demotion-immune.
        ("starred", 3.0_f64, true)
    };

    let n = conn
        .execute(
            "UPDATE vocabulary SET source = ?1, weight = ?2, last_used = ?3
           WHERE user_id = ?4 AND term = ?5",
            params![
                new_source,
                new_weight,
                now_ms(),
                state.default_user_id.as_str(),
                trimmed
            ],
        )
        .unwrap_or(0);
    info!("[vocab] toggle_star term={trimmed:?} → source={new_source} starred={starred} rows={n}");
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    (StatusCode::OK, Json(StarResponse { starred }))
}
