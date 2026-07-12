//! Batch profiling worker: claims coalesced per-user jobs, runs one DeepSeek-V4-Flash
//! pass per app-bucket over the window, and split-applies the result — per-bucket style
//! to the overlay table, global KB to `runtime_user_profiles`. Aliases/terms are left to
//! the reviewed per-edit pipeline. Auto-applies confident deltas, shadows the rest.

use std::time::Duration;

use serde_json::{Value, json};
use tracing::{info, warn};

use crate::AppState;
use crate::profile::bucket::{self, Bucket};
use crate::profile::store;
use crate::profile::updater::batch::{self, BucketedRun};
use crate::profile::updater::deepseek;
use crate::profile::updater::types::{BatchProfileResponse, BatchRunInput};
use crate::profile::updater::validator::regenerate_markdown_from_json;

const TICK_SECS: u64 = 5;
/// Jobs claimed per tick — also the concurrency bound (each is a distinct user).
const CLAIM_LIMIT: i64 = 8;
/// Auto-apply floor: the model must recommend apply AND clear this confidence.
const APPLY_CONFIDENCE: f64 = 0.6;
/// Per-field caps sent to the model, to bound tokens.
const RAW_CAP: usize = 200;
const TEXT_CAP: usize = 300;

pub fn start_batch_worker(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            if let Err(e) = batch::reap_stuck(&state.db).await {
                warn!("[profile-batch] reap failed: {e}");
            }
            let jobs = match batch::claim_jobs(&state.db, CLAIM_LIMIT).await {
                Ok(jobs) => jobs,
                Err(e) => {
                    warn!("[profile-batch] claim failed: {e}");
                    continue;
                }
            };
            if jobs.is_empty() {
                continue;
            }
            // Each job is a distinct user; process them concurrently. The claim limit
            // bounds fan-out.
            let mut handles = Vec::with_capacity(jobs.len());
            for job in jobs {
                let st = state.clone();
                handles.push(tokio::spawn(async move { process_job(&st, job).await }));
            }
            for h in handles {
                let _ = h.await;
            }
        }
    });
    info!(
        "[profile-batch] started ({TICK_SECS}s poll, threshold={})",
        batch::runs_per_batch()
    );
}

async fn process_job(state: &AppState, job: batch::BatchJobRow) {
    let account_id = job.account_id;
    let org_scope = job.org_scope;

    let since = match batch::last_window_mark(&state.db, account_id, org_scope).await {
        Ok(m) => m,
        Err(e) => {
            let _ = batch::finish_job(
                &state.db,
                job.id,
                "failed",
                None,
                0,
                None,
                None,
                None,
                None,
                Some(&e.to_string()),
            )
            .await;
            return;
        }
    };
    let window = match batch::collect_window(&state.db, account_id, org_scope, since).await {
        Ok(w) => w,
        Err(e) => {
            let _ = batch::finish_job(
                &state.db,
                job.id,
                "failed",
                None,
                0,
                None,
                None,
                None,
                None,
                Some(&e.to_string()),
            )
            .await;
            return;
        }
    };

    let run_count = window.len() as i32;
    let window_from = window.first().map(|r| r.run.created_at);
    let window_to = window.last().map(|r| r.run.created_at);

    if window.is_empty() {
        let _ = batch::finish_job(
            &state.db,
            job.id,
            "skipped",
            Some("empty"),
            0,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        let _ = batch::bump_run_stats(&state.db, account_id, org_scope, "skipped", true).await;
        return;
    }

    // Distinct genuinely-unknown apps (Default bucket, not in the static table) for the
    // model to classify into the fixed enum.
    let mut unknown_apps: Vec<String> = Vec::new();
    for r in &window {
        if r.bucket == Bucket::Default
            && let Some(app) = &r.run.target_app
            && !app.trim().is_empty()
            && bucket::static_bucket(app).is_none()
            && !unknown_apps.contains(app)
        {
            unknown_apps.push(app.clone());
        }
    }

    // Which buckets in this window are worth a DeepSeek call (edited, or not yet learned).
    let mut signal_buckets: Vec<Bucket> = Vec::new();
    for bucket in Bucket::ALL {
        let runs: Vec<&BucketedRun> = window.iter().filter(|r| r.bucket == bucket).collect();
        if runs.is_empty() {
            continue;
        }
        let edited_any = runs.iter().any(|r| r.was_edited);
        if bucket_has_signal(state, account_id, org_scope, bucket, edited_any).await {
            signal_buckets.push(bucket);
        }
    }
    if signal_buckets.is_empty() {
        info!(
            "[profile-batch] account={} skipped — no signal across {} runs (buckets already stable)",
            account_id, run_count
        );
        let _ = batch::finish_job(
            &state.db,
            job.id,
            "skipped",
            Some("no_signal"),
            run_count,
            window_from,
            window_to,
            None,
            None,
            None,
        )
        .await;
        let _ = batch::bump_run_stats(&state.db, account_id, org_scope, "skipped", true).await;
        return;
    }
    info!(
        "[profile-batch] account={} analyzing {} runs across buckets {:?}",
        account_id,
        run_count,
        signal_buckets
            .iter()
            .map(|b| b.as_key())
            .collect::<Vec<_>>()
    );

    let global_summary = load_global_summary(state, account_id, org_scope).await;
    let started = std::time::Instant::now();
    let mut applied_any = false;
    let mut kb_deltas: Vec<BatchProfileResponse> = Vec::new();

    for bucket in signal_buckets {
        let runs: Vec<&BucketedRun> = window.iter().filter(|r| r.bucket == bucket).collect();

        let overlay_md = bucket::get_bucket_profile(&state.db, account_id, org_scope, bucket)
            .await
            .ok()
            .flatten()
            .map(|r| r.profile_markdown)
            .unwrap_or_default();

        let run_inputs: Vec<BatchRunInput> = runs
            .iter()
            .map(|r| BatchRunInput {
                was_edited: r.was_edited,
                raw_transcript: r
                    .run
                    .raw_transcript
                    .as_deref()
                    .map(|s| said_core::text::truncate_utf8(s, RAW_CAP).to_string()),
                polished_output: said_core::text::truncate_utf8(
                    r.run.polished_output.as_deref().unwrap_or(""),
                    TEXT_CAP,
                )
                .to_string(),
                final_text: said_core::text::truncate_utf8(
                    r.run.final_text.as_deref().unwrap_or(""),
                    TEXT_CAP,
                )
                .to_string(),
            })
            .collect();

        let request = json!({
            "bucket": bucket.as_key(),
            "runs": run_inputs,
            "current_bucket_style": overlay_md,
            "global_summary": global_summary,
            "unknown_apps": if bucket == Bucket::Default { unknown_apps.clone() } else { Vec::new() },
        });

        let resp = match deepseek::call_deepseek_batch_profile(state, &request).await {
            Ok((resp, _latency)) => resp,
            Err(e) => {
                warn!(
                    "[profile-batch] bucket={} deepseek failed: {e}",
                    bucket.as_key()
                );
                continue;
            }
        };

        // App classifications are safe to persist regardless of the apply gate.
        for s in &resp.app_bucket_suggestions {
            if let Some(b) = Bucket::from_key(&s.bucket)
                && b != Bucket::Default
                && !s.app_key.trim().is_empty()
            {
                let _ = bucket::upsert_app_bucket(&state.db, &s.app_key, b, "agent", s.confidence)
                    .await;
            }
        }

        if !(resp.apply && resp.confidence >= APPLY_CONFIDENCE) {
            info!(
                "[profile-batch] shadow bucket={} account={} confidence={:.2}",
                bucket.as_key(),
                account_id,
                resp.confidence
            );
            continue;
        }

        // Per-bucket style -> overlay.
        let overlay_json = json!({
            "style": resp.style_updates,
            "speech_patterns": resp.speech_patterns,
        });
        let overlay_markdown = render_bucket_overlay_markdown(bucket, &overlay_json);
        if let Err(e) = bucket::upsert_bucket_profile(
            &state.db,
            account_id,
            org_scope,
            bucket,
            overlay_json,
            overlay_markdown,
        )
        .await
        {
            warn!(
                "[profile-batch] overlay upsert failed bucket={}: {e}",
                bucket.as_key()
            );
            continue;
        }
        applied_any = true;

        if resp.user_background.is_some()
            || !resp.add_domains.is_empty()
            || !resp.add_focus_areas.is_empty()
        {
            kb_deltas.push(resp);
        }
    }

    // Fold accumulated global KB deltas into the shared profile once.
    if applied_any && !kb_deltas.is_empty() {
        apply_global_kb(state, account_id, org_scope, &kb_deltas).await;
    }

    let latency = started.elapsed().as_millis() as i64;
    let outcome = if applied_any { "applied" } else { "shadow" };
    let _ = batch::finish_job(
        &state.db,
        job.id,
        outcome,
        None,
        run_count,
        window_from,
        window_to,
        Some(latency),
        None,
        None,
    )
    .await;
    let _ = batch::bump_run_stats(&state.db, account_id, org_scope, outcome, false).await;
    info!(
        "[profile-batch] job done account={} runs={} outcome={} latency_ms={}",
        account_id, run_count, outcome, latency
    );
}

/// A bucket has signal if any run in it was edited, or its overlay isn't yet established.
async fn bucket_has_signal(
    state: &AppState,
    account_id: uuid::Uuid,
    org_scope: uuid::Uuid,
    bucket: Bucket,
    edited_any: bool,
) -> bool {
    if edited_any {
        return true;
    }
    match bucket::get_bucket_profile(&state.db, account_id, org_scope, bucket).await {
        Ok(Some(row)) => row.version <= 1, // established once it has been rebuilt at least once
        _ => true,                         // no overlay yet -> worth establishing a baseline
    }
}

/// Short markdown summary of the global profile, to prime per-bucket calls.
async fn load_global_summary(
    state: &AppState,
    account_id: uuid::Uuid,
    org_scope: uuid::Uuid,
) -> String {
    store::get_profile_with_fallback(&state.db, account_id, org_scope)
        .await
        .ok()
        .flatten()
        .map(|r| said_core::text::truncate_utf8(&r.profile_markdown, 1200).to_string())
        .unwrap_or_default()
}

/// Merge KB deltas (background/domains/focus) into the global profile_json, preserving
/// alias/term fields owned by the reviewed pipeline, then regenerate markdown + persist.
async fn apply_global_kb(
    state: &AppState,
    account_id: uuid::Uuid,
    org_scope: uuid::Uuid,
    deltas: &[BatchProfileResponse],
) {
    let current = store::get_profile(&state.db, account_id, org_scope)
        .await
        .ok()
        .flatten();
    let mut json = current
        .as_ref()
        .map(|r| r.profile_json.clone())
        .unwrap_or_else(|| json!({}));
    if !json.is_object() {
        json = json!({});
    }
    let obj = json.as_object_mut().expect("object");

    for resp in deltas {
        if let Some(bg) = &resp.user_background {
            obj.insert(
                "user_background".into(),
                json!({ "summary": bg.summary, "evidence": bg.evidence }),
            );
        }
        merge_named(
            obj,
            "domains",
            "name",
            resp.add_domains
                .iter()
                .map(|d| json!({"name": d.name, "weight": d.weight, "evidence": d.evidence})),
        );
        merge_named(
            obj,
            "focus_areas",
            "area",
            resp.add_focus_areas
                .iter()
                .map(|f| json!({"area": f.area, "weight": f.weight, "evidence": f.evidence})),
        );
    }

    let markdown = regenerate_markdown_from_json(&json);
    if store::validate_profile_sizes(&json, &markdown).is_err() {
        warn!("[profile-batch] global KB skipped for account={account_id}: size cap");
        return;
    }
    let patch = store::ProfilePatch {
        profile_json: Some(json),
        profile_markdown: Some(markdown),
        mark_dirty: false,
        source: "batch",
    };
    if let Err(e) = store::upsert_profile_patch(&state.db, account_id, org_scope, patch).await {
        warn!("[profile-batch] global KB upsert failed account={account_id}: {e}");
    }
}

/// Append new entries to a named array field, de-duplicating on `key`, capped at 12.
fn merge_named(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
    key: &str,
    additions: impl Iterator<Item = Value>,
) {
    let mut arr: Vec<Value> = obj
        .get(field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for add in additions {
        let name = add.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        let exists = arr
            .iter()
            .any(|e| e.get(key).and_then(|v| v.as_str()) == Some(name));
        if !exists && !name.is_empty() {
            arr.push(add);
        }
    }
    arr.truncate(12);
    if !arr.is_empty() {
        obj.insert(field.to_string(), Value::Array(arr));
    }
}

/// Render a bucket overlay's style json into the injected conditional block.
fn render_bucket_overlay_markdown(bucket: Bucket, overlay_json: &Value) -> String {
    let mut lines = vec![format!("When dictating in {} apps:", bucket.as_key())];
    if let Some(style) = overlay_json.get("style").and_then(|v| v.as_array()) {
        for s in style.iter().take(8) {
            if let Some(pref) = s.get("preference").and_then(|v| v.as_str()) {
                lines.push(format!("- {pref}"));
            }
        }
    }
    if let Some(sp) = overlay_json
        .get("speech_patterns")
        .and_then(|v| v.as_array())
    {
        for s in sp.iter().take(4) {
            if let Some(p) = s.get("pattern").and_then(|v| v.as_str()) {
                lines.push(format!("- {p}"));
            }
        }
    }
    said_core::text::truncate_utf8(&lines.join("\n"), 1200).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_markdown_names_the_bucket_and_prefs() {
        let json = json!({
            "style": [{"category": "greeting", "preference": "always add a greeting", "evidence": "x"}],
            "speech_patterns": [{"pattern": "full sentences", "evidence": "y"}],
        });
        let md = render_bucket_overlay_markdown(Bucket::Messaging, &json);
        assert!(md.contains("messaging"));
        assert!(md.contains("always add a greeting"));
        assert!(md.contains("full sentences"));
    }

    #[test]
    fn merge_named_dedups_on_key() {
        let mut obj = serde_json::Map::new();
        merge_named(
            &mut obj,
            "domains",
            "name",
            [json!({"name": "rust"})].into_iter(),
        );
        merge_named(
            &mut obj,
            "domains",
            "name",
            [json!({"name": "rust"}), json!({"name": "audio"})].into_iter(),
        );
        let arr = obj.get("domains").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }
}
