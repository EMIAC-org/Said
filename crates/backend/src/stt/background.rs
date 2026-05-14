use tracing::{debug, info};

use crate::{
    AppState,
    llm::alias_review::{self, AliasReviewInput},
    store::{prefs::get_prefs, stt_replacements, vocab_embeddings, vocabulary},
};

use super::bias;

pub fn spawn_alias_review(
    state: AppState,
    transcript_form: String,
    correct_form: String,
    output_language: String,
) {
    tokio::spawn(async move {
        let _guard = crate::bg_task_guard();
        info!(
            "[bg] alias-review {transcript_form:?}→{correct_form:?} started (active={})",
            crate::BG_TASK_COUNT.load(std::sync::atomic::Ordering::Relaxed)
        );
        let t0 = std::time::Instant::now();
        review_one_alias(
            state,
            transcript_form.clone(),
            correct_form,
            output_language,
        )
        .await;
        info!(
            "[bg] alias-review {transcript_form:?} done in {}ms",
            t0.elapsed().as_millis()
        );
    });
}

pub async fn run_pending_alias_reviews(state: AppState, limit: usize) {
    let user_id = state.default_user_id.to_string();
    let candidates = stt_replacements::review_candidates(&state.pool, &user_id, limit);
    if candidates.is_empty() {
        return;
    }
    info!(
        "[alias-review] reviewing {} pending/contradicted alias candidate(s)",
        candidates.len()
    );
    for rule in candidates {
        let language = rule.language.clone().unwrap_or_default();
        review_one_alias(
            state.clone(),
            rule.transcript_form.clone(),
            rule.correct_form.clone(),
            language,
        )
        .await;
    }
}

async fn review_one_alias(
    state: AppState,
    transcript_form: String,
    correct_form: String,
    output_language: String,
) {
    let user_id = state.default_user_id.to_string();
    let Some(rule) = stt_replacements::get_for_language(
        &state.pool,
        &user_id,
        &transcript_form,
        &correct_form,
        &output_language,
    ) else {
        return;
    };
    let Some(canonical) = vocabulary::get_term(&state.pool, &user_id, &correct_form) else {
        return;
    };

    let deterministic = bias::deterministic_export_tier(&canonical, &rule);
    if rule.review_status == stt_replacements::ReviewStatus::Blocked
        && rule.export_tier == stt_replacements::ExportTier::Blocked
    {
        return;
    }

    let prefs = get_prefs(&state.pool, &state.default_user_id);
    let groq_key = prefs
        .as_ref()
        .and_then(|p| p.groq_api_key.clone())
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .unwrap_or_default();
    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if groq_key.is_empty() && openai_key.is_empty() {
        let _ = stt_replacements::update_export_metadata(
            &state.pool,
            &user_id,
            &transcript_form,
            &correct_form,
            deterministic,
            stt_replacements::ReviewStatus::Skipped,
            Some("No review keys available; kept deterministic tier."),
            &output_language,
        );
        return;
    }

    let examples = vocab_embeddings::support_example_texts(&state.pool, &user_id, &correct_form, 4);
    let input = AliasReviewInput {
        canonical: &correct_form,
        alias: &transcript_form,
        term_type: canonical.term_type.as_deref(),
        current_tier: deterministic,
        contradiction_count: rule.contradiction_count,
        use_count: rule.use_count,
        weight: rule.weight,
        example_contexts: &examples,
    };
    let decision =
        alias_review::review_alias(&state.http_client, &groq_key, &openai_key, &input).await;
    let (tier, status, reason) = match decision {
        Some(d) => (d.export_tier, d.review_status, d.reason),
        None => (
            deterministic,
            stt_replacements::ReviewStatus::Skipped,
            "Alias review unavailable; kept deterministic tier.".to_string(),
        ),
    };
    let updated = stt_replacements::update_export_metadata(
        &state.pool,
        &user_id,
        &transcript_form,
        &correct_form,
        tier,
        status,
        Some(&reason),
        &output_language,
    );
    if updated {
        debug!(
            "[alias-review] stored tier={} status={} for {:?} -> {:?}",
            tier.as_str(),
            status.as_str(),
            transcript_form,
            correct_form
        );
    }
}
