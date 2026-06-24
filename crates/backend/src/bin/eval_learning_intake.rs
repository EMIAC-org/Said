//! Grand intake + learning pipeline evaluator.
//!
//! This binary reads recent real AirNote edit evidence from the user's local
//! SQLite DB, replays it through the real backend classify/confirm routes on a
//! temporary DB, then verifies the persisted vocabulary/STT aliases and a
//! second-pass alias application.

use axum::Router;
use clap::Parser;
use reqwest::StatusCode;
use rusqlite::{Connection, params};
use said_backend::{
    AppState,
    store::{
        self,
        history::{self, InsertRecording},
        profile_summary,
        stt_replacements::{self, ApplyResult},
        vocabulary,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const SECRET: &str = "learning-intake-grand-secret";
const CORE_EVAL_TERMS: &[&str] = &[
    "EMIAC",
    "Macobs",
    "Kafka",
    "ZooKeeper",
    "Sentry",
    "AirNote",
    "Deepgram",
    "DeepSeek",
    "DeepInfra",
    "Cerebras",
    "Postgres",
    "SQLite",
    "Qdrant",
    "Groq",
];

#[derive(Parser, Debug)]
#[command(
    name = "eval-learning-intake",
    about = "Replay real AirNote edit history through classify -> confirm -> alias apply"
)]
struct Args {
    /// Read-only source AirNote SQLite DB.
    #[arg(long)]
    source_db: Option<PathBuf>,

    /// Maximum recent edit events to replay from the source DB.
    #[arg(long, default_value_t = 18)]
    max_history_cases: usize,

    /// Keep the temporary eval SQLite DB after the run.
    #[arg(long)]
    keep_db: bool,

    /// Stop on the first failing case.
    #[arg(long)]
    fail_fast: bool,

    /// Delay between LLM-backed cases to stay within provider limits.
    #[arg(long, default_value_t = 1_750)]
    case_delay_ms: u64,

    /// JSON report path.
    #[arg(long, default_value = ".context/learning-intake-grand/latest.json")]
    json_out: PathBuf,

    /// Markdown report path.
    #[arg(long, default_value = ".context/learning-intake-grand/latest.md")]
    md_out: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct EvalCase {
    id: String,
    source: String,
    description: String,
    raw_transcript: String,
    airnote_output: String,
    user_kept: String,
}

#[derive(Debug, Deserialize)]
struct ClassifyResponse {
    #[serde(rename = "class")]
    class_name: String,
    reason: String,
    learned: bool,
    notify: bool,
    promoted_count: usize,
    #[serde(default)]
    changes: Vec<Change>,
    #[serde(default)]
    review_candidates: Vec<ReviewCandidate>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct ConfirmItem {
    original: String,
    corrected: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Change {
    original: String,
    corrected: String,
    reason: String,
    should_learn: bool,
    confidence: f64,
    #[serde(default)]
    skip_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReviewCandidate {
    original: String,
    corrected: String,
    term_type: String,
    learnable: bool,
    tag: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ConfirmBatchResponse {
    learned_count: usize,
    blocked_count: usize,
    learned_terms: Vec<String>,
    server_owned: bool,
}

#[derive(Debug, Serialize)]
struct StoredAlias {
    transcript_form: String,
    correct_form: String,
    weight: f64,
    use_count: i64,
    language: Option<String>,
    review_status: String,
    export_tier: String,
}

#[derive(Debug, Serialize)]
struct StoredTerm {
    term: String,
    source: String,
    weight: f64,
    use_count: i64,
    language: Option<String>,
    term_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct AliasProbe {
    input: String,
    output: String,
    matched: bool,
    expected: String,
    match_count: usize,
    matches: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProfileCheck {
    kind: String,
    needle: String,
    matched: bool,
}

#[derive(Debug, Serialize)]
struct CaseReport {
    id: String,
    source: String,
    description: String,
    passed: bool,
    classify_status: u16,
    classify_latency_ms: u128,
    confirm_status: Option<u16>,
    confirm_latency_ms: Option<u128>,
    class_name: Option<String>,
    classify_reason: Option<String>,
    learned: Option<bool>,
    notify: Option<bool>,
    promoted_count: Option<usize>,
    changes: Vec<Change>,
    review_candidates: Vec<ReviewCandidate>,
    approved_items: Vec<ConfirmItem>,
    confirm_response: Option<ConfirmBatchResponse>,
    stored_terms: Vec<StoredTerm>,
    stored_aliases: Vec<StoredAlias>,
    alias_probes: Vec<AliasProbe>,
    profile_version: Option<i64>,
    profile_chars: Option<usize>,
    profile_checks: Vec<ProfileCheck>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvalReport {
    source_db: PathBuf,
    eval_db: PathBuf,
    total_cases: usize,
    passed_cases: usize,
    failed_cases: usize,
    stored_alias_total: usize,
    stored_term_total: usize,
    cases: Vec<CaseReport>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    said_core::load_env();
    unsafe {
        std::env::set_var("AIRNOTE_DISABLE_ONNX_RETRAIN", "1");
    }

    let args = Args::parse();
    let source_db = args.source_db.clone().unwrap_or_else(default_source_db);
    if !source_db.exists() {
        anyhow::bail!("source DB does not exist: {}", source_db.display());
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| {
                "said_backend::routes::classify=info,said_backend::routes::confirm=info,said_backend::llm::alias_safety=info,said_backend::store::stt_replacements=info".to_string()
            }),
        )
        .try_init();

    let eval_db = eval_db_path()?;
    let pool = store::open(&eval_db);
    let user_id = store::ensure_default_user(&pool);
    seed_vocab_from_source(&source_db, &pool, &user_id)?;

    let state = AppState {
        pool: pool.clone(),
        shared_secret: Arc::new(SECRET.to_string()),
        default_user_id: Arc::new(user_id.clone()),
        prefs_cache: Arc::new(RwLock::new(None)),
        lexicon_cache: Arc::new(RwLock::new(None)),
        live_server_runtime_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
        http_client: reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()?,
        watchdog: Arc::new(said_backend::watchdog::WatchdogState::new()),
    };

    let cases = load_cases_from_history(&source_db, args.max_history_cases)?;
    if cases.is_empty() {
        anyhow::bail!(
            "no usable edit history cases found in {}",
            source_db.display()
        );
    }

    let base_url = spawn_eval_server(said_backend::router_with_state(state)).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()?;

    println!("Source DB: {}", source_db.display());
    println!("Eval DB:   {}", eval_db.display());
    println!("Backend:   {base_url}");
    println!("Cases:     {}\n", cases.len());

    let mut reports = Vec::new();
    for (idx, case) in cases.iter().enumerate() {
        let report = run_case(&client, &base_url, &pool, &user_id, case).await;
        print_case(&report);
        let failed = !report.passed;
        reports.push(report);
        if failed && args.fail_fast {
            break;
        }
        if idx + 1 < cases.len() {
            tokio::time::sleep(Duration::from_millis(args.case_delay_ms)).await;
        }
    }

    let passed_cases = reports.iter().filter(|r| r.passed).count();
    let failed_cases = reports.len().saturating_sub(passed_cases);
    let stored_alias_total = load_aliases(&pool, &user_id).len();
    let stored_term_total = load_terms(&pool, &user_id).len();
    let report = EvalReport {
        source_db: source_db.clone(),
        eval_db: eval_db.clone(),
        total_cases: reports.len(),
        passed_cases,
        failed_cases,
        stored_alias_total,
        stored_term_total,
        cases: reports,
    };

    if let Some(parent) = args.json_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = args.md_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.json_out, serde_json::to_string_pretty(&report)?)?;
    std::fs::write(&args.md_out, render_markdown(&report))?;

    println!(
        "\nSummary: {passed_cases}/{} passed, aliases={}, terms={}",
        report.total_cases, stored_alias_total, stored_term_total
    );
    println!("JSON: {}", args.json_out.display());
    println!("Markdown: {}", args.md_out.display());

    if !args.keep_db {
        let _ = std::fs::remove_file(&eval_db);
        let _ = std::fs::remove_file(format!("{}-wal", eval_db.display()));
        let _ = std::fs::remove_file(format!("{}-shm", eval_db.display()));
    }

    if failed_cases > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn spawn_eval_server(router: Router) -> anyhow::Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            eprintln!("eval server failed: {err}");
        }
    });
    Ok(format!("http://{addr}"))
}

async fn run_case(
    client: &reqwest::Client,
    base_url: &str,
    pool: &store::DbPool,
    user_id: &str,
    case: &EvalCase,
) -> CaseReport {
    let recording_id = format!("grand-{}-{}", safe_id(&case.id), uuid::Uuid::new_v4());
    let _ = history::insert_recording(
        pool,
        InsertRecording {
            id: &recording_id,
            user_id,
            transcript: &case.raw_transcript,
            polished: &case.airnote_output,
            word_count: case.airnote_output.split_whitespace().count() as i64,
            recording_seconds: estimate_seconds(&case.raw_transcript),
            model_used: "eval-learning-intake",
            confidence: Some(0.92),
            transcribe_ms: Some(0),
            embed_ms: Some(0),
            polish_ms: Some(0),
            target_app: Some("eval-learning-intake"),
            source: "eval",
            audio_id: None,
            enriched_transcript: None,
            raw_transcript: Some(&case.raw_transcript),
            local_corrected_transcript: Some(&case.airnote_output),
            polished_output: Some(&case.airnote_output),
        },
    );

    let classify_payload = serde_json::json!({
        "recording_id": recording_id,
        "ai_output": case.airnote_output,
        "user_kept": case.user_kept,
        "capture_method": "ax",
        "time_since_paste_ms": 15_000,
        "app_switched": false,
        "matches_clipboard": false,
        "client_run_id": case.id,
    });

    let started = Instant::now();
    let classify_resp = client
        .post(format!("{base_url}/v1/classify-edit"))
        .bearer_auth(SECRET)
        .json(&classify_payload)
        .send()
        .await;
    let classify_latency_ms = started.elapsed().as_millis();

    let classify_resp = match classify_resp {
        Ok(resp) => resp,
        Err(err) => {
            return failed_case(
                case,
                0,
                classify_latency_ms,
                format!("classify request failed: {err}"),
            );
        }
    };
    let classify_status = classify_resp.status();
    let parsed = match classify_resp.json::<ClassifyResponse>().await {
        Ok(parsed) => parsed,
        Err(err) => {
            return failed_case(
                case,
                classify_status.as_u16(),
                classify_latency_ms,
                format!("classify JSON parse failed: {err}"),
            );
        }
    };

    let approved_items = approval_items(&parsed);
    let mut confirm_status = None;
    let mut confirm_latency_ms = None;
    let mut confirm_response = None;

    if !approved_items.is_empty() {
        let confirm_payload = serde_json::json!({
            "recording_id": recording_id,
            "items": approved_items,
        });
        let started = Instant::now();
        match client
            .post(format!("{base_url}/v1/confirm-batch"))
            .bearer_auth(SECRET)
            .json(&confirm_payload)
            .send()
            .await
        {
            Ok(resp) => {
                confirm_status = Some(resp.status().as_u16());
                confirm_latency_ms = Some(started.elapsed().as_millis());
                match resp.json::<ConfirmBatchResponse>().await {
                    Ok(body) => confirm_response = Some(body),
                    Err(err) => {
                        return failed_from_classify(
                            case,
                            classify_status.as_u16(),
                            classify_latency_ms,
                            parsed,
                            approved_items,
                            Some(format!("confirm JSON parse failed: {err}")),
                            confirm_status,
                            confirm_latency_ms,
                            confirm_response,
                        );
                    }
                }
            }
            Err(err) => {
                return failed_from_classify(
                    case,
                    classify_status.as_u16(),
                    classify_latency_ms,
                    parsed,
                    approved_items,
                    Some(format!("confirm request failed: {err}")),
                    confirm_status,
                    confirm_latency_ms,
                    confirm_response,
                );
            }
        }
    }

    let stored_terms = load_terms(pool, user_id);
    let stored_aliases = load_aliases(pool, user_id);
    let alias_probes = run_alias_probes(pool, user_id, &approved_items);
    let profile = profile_summary::ensure_current(pool, user_id);
    let profile_checks = run_profile_checks(profile.as_ref(), &approved_items, &stored_aliases);
    let passed = classify_status == StatusCode::OK
        && approved_items.iter().all(|item| {
            item.original.trim().is_empty()
                || stored_aliases.iter().any(|alias| {
                    norm(&alias.transcript_form) == norm(&item.original)
                        && norm(&alias.correct_form) == norm(&item.corrected)
                        && alias.review_status == "approved"
                })
        })
        && alias_probes.iter().all(|probe| probe.matched)
        && profile_checks.iter().all(|check| check.matched);

    CaseReport {
        id: case.id.clone(),
        source: case.source.clone(),
        description: case.description.clone(),
        passed,
        classify_status: classify_status.as_u16(),
        classify_latency_ms,
        confirm_status,
        confirm_latency_ms,
        class_name: Some(parsed.class_name),
        classify_reason: Some(parsed.reason),
        learned: Some(parsed.learned),
        notify: Some(parsed.notify),
        promoted_count: Some(parsed.promoted_count),
        changes: parsed.changes,
        review_candidates: parsed.review_candidates,
        approved_items,
        confirm_response,
        stored_terms,
        stored_aliases,
        alias_probes,
        profile_version: profile.as_ref().map(|summary| summary.version),
        profile_chars: profile
            .as_ref()
            .map(|summary| summary.profile_markdown.chars().count()),
        profile_checks,
        error: None,
    }
}

fn failed_case(case: &EvalCase, status: u16, latency_ms: u128, error: String) -> CaseReport {
    CaseReport {
        id: case.id.clone(),
        source: case.source.clone(),
        description: case.description.clone(),
        passed: false,
        classify_status: status,
        classify_latency_ms: latency_ms,
        confirm_status: None,
        confirm_latency_ms: None,
        class_name: None,
        classify_reason: None,
        learned: None,
        notify: None,
        promoted_count: None,
        changes: vec![],
        review_candidates: vec![],
        approved_items: vec![],
        confirm_response: None,
        stored_terms: vec![],
        stored_aliases: vec![],
        alias_probes: vec![],
        profile_version: None,
        profile_chars: None,
        profile_checks: vec![],
        error: Some(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_from_classify(
    case: &EvalCase,
    status: u16,
    latency_ms: u128,
    parsed: ClassifyResponse,
    approved_items: Vec<ConfirmItem>,
    error: Option<String>,
    confirm_status: Option<u16>,
    confirm_latency_ms: Option<u128>,
    confirm_response: Option<ConfirmBatchResponse>,
) -> CaseReport {
    CaseReport {
        id: case.id.clone(),
        source: case.source.clone(),
        description: case.description.clone(),
        passed: false,
        classify_status: status,
        classify_latency_ms: latency_ms,
        confirm_status,
        confirm_latency_ms,
        class_name: Some(parsed.class_name),
        classify_reason: Some(parsed.reason),
        learned: Some(parsed.learned),
        notify: Some(parsed.notify),
        promoted_count: Some(parsed.promoted_count),
        changes: parsed.changes,
        review_candidates: parsed.review_candidates,
        approved_items,
        confirm_response,
        stored_terms: vec![],
        stored_aliases: vec![],
        alias_probes: vec![],
        profile_version: None,
        profile_chars: None,
        profile_checks: vec![],
        error,
    }
}

fn approval_items(parsed: &ClassifyResponse) -> Vec<ConfirmItem> {
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for candidate in &parsed.review_candidates {
        if candidate.learnable && !candidate.corrected.trim().is_empty() {
            let item = ConfirmItem {
                original: candidate.original.trim().to_string(),
                corrected: candidate.corrected.trim().to_string(),
            };
            let original_norm = norm(&item.original);
            let corrected_norm = norm(&item.corrected);
            if !original_norm.is_empty() && original_norm == corrected_norm {
                continue;
            }
            if seen.insert((original_norm, corrected_norm)) {
                items.push(item);
            }
        }
    }
    items
}

fn run_alias_probes(
    pool: &store::DbPool,
    user_id: &str,
    approved_items: &[ConfirmItem],
) -> Vec<AliasProbe> {
    let rules = stt_replacements::load_for_language(pool, user_id, "hinglish");
    let approved_terms = approved_items
        .iter()
        .filter(|item| !item.original.trim().is_empty())
        .map(|item| norm(&item.corrected))
        .collect::<HashSet<_>>();
    let mut probes = Vec::new();
    for alias in rules.iter().filter(|r| {
        r.review_status == stt_replacements::ReviewStatus::Approved
            && approved_terms.contains(&norm(&r.correct_form))
    }) {
        let variants = probe_variants(&alias.transcript_form);
        for variant in variants {
            let input = format!("{variant} ka production issue check karna hai");
            let result = stt_replacements::apply_exact_safe(&input, &rules);
            probes.push(alias_probe(input, result, &alias.correct_form));
        }
    }
    probes
}

fn alias_probe(input: String, result: ApplyResult, expected: &str) -> AliasProbe {
    let output = result.text;
    let matched = output
        .to_ascii_lowercase()
        .contains(&expected.to_ascii_lowercase());
    let matches = result
        .matches
        .iter()
        .map(|m| {
            format!(
                "{}->{}/{}",
                m.transcript_form,
                m.correct_form,
                m.kind.as_str()
            )
        })
        .collect();
    AliasProbe {
        input,
        output,
        matched,
        expected: expected.to_string(),
        match_count: result.matches.len(),
        matches,
    }
}

fn run_profile_checks(
    profile: Option<&profile_summary::CachedProfileSummary>,
    approved_items: &[ConfirmItem],
    stored_aliases: &[StoredAlias],
) -> Vec<ProfileCheck> {
    if approved_items.is_empty() {
        return vec![];
    }
    let haystack = profile
        .map(|summary| summary.profile_markdown.as_str())
        .unwrap_or_default();
    let mut checks = Vec::new();
    let mut seen = HashSet::new();
    for item in approved_items {
        if !item.corrected.trim().is_empty()
            && seen.insert(("term".to_string(), norm(&item.corrected)))
        {
            checks.push(ProfileCheck {
                kind: "term".to_string(),
                needle: item.corrected.clone(),
                matched: contains_relaxed(haystack, &item.corrected),
            });
        }
        if !item.original.trim().is_empty()
            && stored_aliases.iter().any(|alias| {
                norm(&alias.transcript_form) == norm(&item.original)
                    && norm(&alias.correct_form) == norm(&item.corrected)
                    && alias.review_status == "approved"
            })
            && seen.insert(("alias".to_string(), norm(&item.original)))
        {
            checks.push(ProfileCheck {
                kind: "alias".to_string(),
                needle: item.original.clone(),
                matched: contains_relaxed(haystack, &item.original),
            });
        }
    }
    checks
}

fn contains_relaxed(haystack: &str, needle: &str) -> bool {
    let needle = needle.trim();
    !needle.is_empty()
        && haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
}

fn probe_variants(alias: &str) -> Vec<String> {
    let trimmed = alias.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let mut variants = Vec::new();
    variants.push(trimmed.to_string());
    variants.push(trimmed.to_ascii_uppercase());
    variants.push(trimmed.to_ascii_lowercase());
    if trimmed.contains(' ') {
        variants.push(trimmed.split_whitespace().collect::<String>());
    }
    let mut seen = HashSet::new();
    variants
        .into_iter()
        .filter(|v| seen.insert(v.to_ascii_lowercase()))
        .collect()
}

fn load_terms(pool: &store::DbPool, user_id: &str) -> Vec<StoredTerm> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let mut stmt = match conn.prepare(
        "SELECT term, source, weight, use_count, language, term_type
           FROM vocabulary
          WHERE user_id = ?1
          ORDER BY last_used DESC, term ASC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(StoredTerm {
            term: row.get(0)?,
            source: row.get(1)?,
            weight: row.get(2)?,
            use_count: row.get(3)?,
            language: row.get(4).ok(),
            term_type: row.get(5).ok(),
        })
    })
    .ok()
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

fn load_aliases(pool: &store::DbPool, user_id: &str) -> Vec<StoredAlias> {
    stt_replacements::load_all(pool, user_id)
        .into_iter()
        .map(|alias| StoredAlias {
            transcript_form: alias.transcript_form,
            correct_form: alias.correct_form,
            weight: alias.weight,
            use_count: alias.use_count,
            language: alias.language,
            review_status: alias.review_status.as_str().to_string(),
            export_tier: alias.export_tier.as_str().to_string(),
        })
        .collect()
}

fn seed_vocab_from_source(
    source_db: &Path,
    pool: &store::DbPool,
    user_id: &str,
) -> anyhow::Result<()> {
    let src = Connection::open_with_flags(source_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = src.prepare(
        "SELECT term, COALESCE(language, 'hinglish'), COALESCE(example_context, '')
           FROM vocabulary
          ORDER BY weight DESC, last_used DESC
          LIMIT 100",
    )?;
    let mut seeded = 0;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows.filter_map(Result::ok) {
        let context = if row.2.trim().is_empty() {
            Some("Seeded from local AirNote vocabulary")
        } else {
            Some(row.2.as_str())
        };
        if vocabulary::upsert_for_language_with_context(
            pool,
            user_id,
            &row.0,
            2.0,
            "history_seed",
            &row.1,
            context,
        ) {
            seeded += 1;
        }
    }
    for term in CORE_EVAL_TERMS {
        let _ = vocabulary::upsert_for_language_with_context(
            pool,
            user_id,
            term,
            2.0,
            if seeded == 0 {
                "fallback_seed"
            } else {
                "eval_core_seed"
            },
            "hinglish",
            Some("Core developer/business eval vocabulary"),
        );
    }
    Ok(())
}

fn load_cases_from_history(source_db: &Path, limit: usize) -> anyhow::Result<Vec<EvalCase>> {
    let conn = Connection::open_with_flags(source_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut cases = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT e.id,
                COALESCE(NULLIF(r.raw_transcript, ''), NULLIF(e.transcript, ''), ''),
                COALESCE(NULLIF(r.polished_output, ''), NULLIF(e.ai_output, ''), ''),
                e.user_kept,
                COALESCE(e.target_app, r.target_app, '')
           FROM edit_events e
      LEFT JOIN recordings r ON r.id = e.recording_id
          WHERE length(TRIM(e.user_kept)) > 8
            AND length(TRIM(COALESCE(NULLIF(r.polished_output, ''), NULLIF(e.ai_output, ''), ''))) > 0
            AND TRIM(e.user_kept) != TRIM(COALESCE(NULLIF(r.polished_output, ''), NULLIF(e.ai_output, ''), ''))
          ORDER BY e.timestamp_ms DESC
          LIMIT ?1",
    )?;
    let scan_limit = limit.saturating_mul(6).max(limit).max(12);
    let rows = stmt.query_map(params![scan_limit as i64], |row| {
        Ok(EvalCase {
            id: format!("history-{}", row.get::<_, String>(0)?),
            source: "local_history".to_string(),
            description: format!(
                "Real local edit history{}",
                row.get::<_, String>(4)
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .map(|app| format!(" from {app}"))
                    .unwrap_or_default()
            ),
            raw_transcript: row.get(1)?,
            airnote_output: row.get(2)?,
            user_kept: row.get(3)?,
        })
    })?;
    for case in rows.filter_map(Result::ok) {
        if is_usable_case(&case) {
            cases.push(case);
            if cases.len() >= limit {
                break;
            }
        }
    }
    cases.extend(seed_probe_cases_from_history(&conn)?);
    let mut cases = dedupe_cases(cases);
    if cases.len() < limit {
        let needed = limit.saturating_sub(cases.len());
        cases.extend(seed_multi_swap_mixed_cases(needed.min(24)));
        cases = dedupe_cases(cases);
    }
    if cases.len() < limit {
        let needed = limit.saturating_sub(cases.len());
        cases.extend(seed_mixed_complex_cases(needed));
        cases = dedupe_cases(cases);
    }
    Ok(cases)
}

fn seed_probe_cases_from_history(conn: &Connection) -> anyhow::Result<Vec<EvalCase>> {
    let mut probes = Vec::new();
    let terms = [
        "EMIAC",
        "Emiac",
        "Macobs",
        "MACOBS",
        "Kafka",
        "ZooKeeper",
        "Sentry",
    ];
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(transcript, ''), ''),
                COALESCE(NULLIF(ai_output, ''), ''),
                user_kept
           FROM edit_events
          WHERE user_kept LIKE ?1
          ORDER BY timestamp_ms DESC
          LIMIT 3",
    )?;
    for term in terms {
        let rows = stmt.query_map(params![format!("%{term}%")], |row| {
            Ok(EvalCase {
                id: format!("probe-{}-{}", safe_id(term), uuid::Uuid::new_v4()),
                source: "history_term_probe".to_string(),
                description: format!("History-derived probe for {term}"),
                raw_transcript: row.get(0)?,
                airnote_output: row.get(1)?,
                user_kept: row.get(2)?,
            })
        })?;
        probes.extend(rows.filter_map(Result::ok).filter(is_usable_case));
    }
    Ok(probes)
}

fn dedupe_cases(cases: Vec<EvalCase>) -> Vec<EvalCase> {
    let mut seen = HashSet::new();
    cases
        .into_iter()
        .filter(|case| {
            seen.insert(format!(
                "{}\n{}\n{}",
                norm(&case.raw_transcript),
                norm(&case.airnote_output),
                norm(&case.user_kept)
            ))
        })
        .collect()
}

fn seed_mixed_complex_cases(target_count: usize) -> Vec<EvalCase> {
    if target_count == 0 {
        return vec![];
    }
    let alias_sets: &[(&str, &str, &[&str])] = &[
        ("EMIAC", "AMEAC", &["AMEAC", "MIA", "Emiyak", "Amiac"]),
        (
            "Macobs",
            "MACOPS",
            &["MACOPS", "Mecops", "Macobps", "Makeups"],
        ),
        ("Kafka", "Kaafka", &["Kaafka", "Kaf ka", "Kafqa", "Kaafka"]),
        (
            "ZooKeeper",
            "Zooki",
            &["Zooki", "Zookeeperr", "Zuki par", "Zoo keep her"],
        ),
        (
            "Sentry",
            "Century",
            &["Century", "Sentri", "Saintry", "Entry"],
        ),
        (
            "AirNote",
            "Earnote",
            &["Earnote", "Air not", "A note", "Arnote"],
        ),
        (
            "Deepgram",
            "Deep gram",
            &["Deep gram", "Deep Graham", "Deep graam", "D gram"],
        ),
        (
            "DeepSeek",
            "Deep sick",
            &["Deep sick", "Deep seekh", "Deep seek", "D sick"],
        ),
        (
            "DeepInfra",
            "Deep infra",
            &["Deep infra", "Deep in fra", "Deepin fra", "Deep in front"],
        ),
        (
            "Cerebras",
            "Sara bras",
            &["Sara bras", "Cere brass", "Sare bras", "Seribras"],
        ),
        (
            "Postgres",
            "Post grass",
            &["Post grass", "Post gress", "Postgresq", "Post grez"],
        ),
        (
            "SQLite",
            "CQLite",
            &["CQLite", "SQL light", "Sequelite", "S Q light"],
        ),
        (
            "Qdrant",
            "queue drant",
            &["queue drant", "Q grant", "Cue drant", "Kudrant"],
        ),
        ("Groq", "Grok", &["Grok", "Growq", "G rock", "Grog"]),
    ];
    let templates: &[(&str, &str)] = &[
        (
            "Bhai {wrong} ka production latency check karo, kal client call se pehle dashboard aur retry logs English mein clean update chahiye.",
            "Bhai {right} ka production latency check karo, kal client call se pehle dashboard aur retry logs English mein clean update chahiye.",
        ),
        (
            "Please {wrong} wale deployment notes ko verify kar do, agar cache stale hai to rollback mat karna, pehle root cause likhna.",
            "Please {right} wale deployment notes ko verify kar do, agar cache stale hai to rollback mat karna, pehle root cause likhna.",
        ),
        (
            "Yaar {wrong} integration mein jo webhook retry fail ho raha hai, uska exact run ID nikaal ke finance team ko concise update bhejna.",
            "Yaar {right} integration mein jo webhook retry fail ho raha hai, uska exact run ID nikaal ke finance team ko concise update bhejna.",
        ),
        (
            "Can you check why {wrong} model ka output English plus Hinglish mix mein over-polish ho raha hai, natural tone preserve rehna chahiye.",
            "Can you check why {right} model ka output English plus Hinglish mix mein over-polish ho raha hai, natural tone preserve rehna chahiye.",
        ),
        (
            "Aaj {wrong} ke logs mein jo 500 aa raha hai usko business impact ke saath explain karna, sirf technical dump nahi bhejna.",
            "Aaj {right} ke logs mein jo 500 aa raha hai usko business impact ke saath explain karna, sirf technical dump nahi bhejna.",
        ),
        (
            "For {wrong}, mujhe ek crisp status chahiye: kya broken hai, kisne approve kiya, aur next deploy safely kab kar sakte hain.",
            "For {right}, mujhe ek crisp status chahiye: kya broken hai, kisne approve kiya, aur next deploy safely kab kar sakte hain.",
        ),
    ];
    let mut cases = Vec::new();
    for (term_idx, (right, primary_wrong, wrongs)) in alias_sets.iter().enumerate() {
        for (template_idx, (wrong_template, right_template)) in templates.iter().enumerate() {
            if cases.len() >= target_count {
                return cases;
            }
            let wrong = wrongs
                .get(template_idx % wrongs.len())
                .copied()
                .unwrap_or(primary_wrong);
            let ai = wrong_template
                .replace("{wrong}", wrong)
                .replace("{right}", right);
            let kept = right_template
                .replace("{wrong}", wrong)
                .replace("{right}", right);
            cases.push(EvalCase {
                id: format!("mixed-synthetic-{term_idx:02}-{template_idx:02}"),
                source: "mixed_hinglish_synthetic".to_string(),
                description: format!("English + Hinglish synthetic garble for {right}"),
                raw_transcript: ai.clone(),
                airnote_output: ai,
                user_kept: kept,
            });
        }
    }
    cases
}

fn seed_multi_swap_mixed_cases(target_count: usize) -> Vec<EvalCase> {
    if target_count == 0 {
        return vec![];
    }
    let pairs: &[(&str, &str)] = &[
        (
            "Bhai AMEAC ke MACOPS dashboard mein Zooki par aur Kaafka dono ka retry graph compare karna, phir Saintry run ID ke saath client ko English update bhejna.",
            "Bhai EMIAC ke Macobs dashboard mein ZooKeeper aur Kafka dono ka retry graph compare karna, phir Sentry run ID ke saath client ko English update bhejna.",
        ),
        (
            "Please Deep gram aur Deep sick ke benchmark ko Air not settings mein compare karo, agar Grok latency spike kare to Post grass logs attach kar dena.",
            "Please Deepgram aur DeepSeek ke benchmark ko AirNote settings mein compare karo, agar Groq latency spike kare to Postgres logs attach kar dena.",
        ),
        (
            "Yaar Deep infra se Sara bras fallback tak ka flow check karo, Q grant vector search aur CQLite migration dono same report mein mention karna.",
            "Yaar DeepInfra se Cerebras fallback tak ka flow check karo, Qdrant vector search aur SQLite migration dono same report mein mention karna.",
        ),
        (
            "AMEAC onboarding mein MACOPS user bol raha hai ki Earnote local model slow hai, Deep Graham key missing aur Century warning ek saath aa rahe hain.",
            "EMIAC onboarding mein Macobs user bol raha hai ki AirNote local model slow hai, Deepgram key missing aur Sentry warning ek saath aa rahe hain.",
        ),
        (
            "For Kaafka and Zuki par, mujhe ek business friendly RCA chahiye jisme Deep in fra model, Grok response time aur Post gress connection pool clear ho.",
            "For Kafka and ZooKeeper, mujhe ek business friendly RCA chahiye jisme DeepInfra model, Groq response time aur Postgres connection pool clear ho.",
        ),
        (
            "Can you verify CQLite profile summary, queue drant embeddings, aur AMEAC vocabulary sync, because MACOPS approval card abhi mixed Hinglish mein test ho raha hai.",
            "Can you verify SQLite profile summary, Qdrant embeddings, aur EMIAC vocabulary sync, because Macobs approval card abhi mixed Hinglish mein test ho raha hai.",
        ),
        (
            "Deep seekh prompt mein Zookeeperr aur Kafqa ko overcorrect mat karna, bas Century observability aur Air not retry shortcut ko clean explain karna.",
            "DeepSeek prompt mein ZooKeeper aur Kafka ko overcorrect mat karna, bas Sentry observability aur AirNote retry shortcut ko clean explain karna.",
        ),
        (
            "Mecops finance dashboard ke liye AMEAC report bana do, lekin CQLite aur Post grass migration ka technical detail Hinglish mein simple rakhna.",
            "Macobs finance dashboard ke liye EMIAC report bana do, lekin SQLite aur Postgres migration ka technical detail Hinglish mein simple rakhna.",
        ),
        (
            "Sare bras GPT OSS run mein Deep gram transcript good tha but Deep sick polish ne Grok pricing aur Q grant context ko confuse kar diya.",
            "Cerebras GPT OSS run mein Deepgram transcript good tha but DeepSeek polish ne Groq pricing aur Qdrant context ko confuse kar diya.",
        ),
        (
            "Amiac team ko bolo MACOPS inventory aur Kaaf ka queue dono check karein, Zooki par dependency agar down hai to Saintry mein incident daal do.",
            "EMIAC team ko bolo Macobs inventory aur Kafka queue dono check karein, ZooKeeper dependency agar down hai to Sentry mein incident daal do.",
        ),
        (
            "Air not app ke release note mein Deep in front, Sara bras, aur Post grez changes ko alag bullet mein likho, taaki business users confuse na hon.",
            "AirNote app ke release note mein DeepInfra, Cerebras, aur Postgres changes ko alag bullet mein likho, taaki business users confuse na hon.",
        ),
        (
            "MIA ke client update mein Makeups ko correct karna, Kaafka aur Zoo keep her ka failure mention karna, aur Century alert ka screenshot attach karna.",
            "EMIAC ke client update mein Macobs ko correct karna, Kafka aur ZooKeeper ka failure mention karna, aur Sentry alert ka screenshot attach karna.",
        ),
        (
            "Please Deep graam key rotation, Deep sick profile updater, aur CQLite local cache ko same deployment checklist mein daal do, par tone natural Hinglish rehna chahiye.",
            "Please Deepgram key rotation, DeepSeek profile updater, aur SQLite local cache ko same deployment checklist mein daal do, par tone natural Hinglish rehna chahiye.",
        ),
        (
            "G rock streaming fast hai but Earnote paste path mein Zuki par aur Kaafqa words toot rahe hain, isliye direct alias probes zaroor run karna.",
            "Groq streaming fast hai but AirNote paste path mein ZooKeeper aur Kafka words toot rahe hain, isliye direct alias probes zaroor run karna.",
        ),
        (
            "Deepin fra benchmark ke baad Sara bras fallback aur Q grant retrieval dono verify karna, warna AMEAC demo mein MACOPS names phir garble ho jayenge.",
            "DeepInfra benchmark ke baad Cerebras fallback aur Qdrant retrieval dono verify karna, warna EMIAC demo mein Macobs names phir garble ho jayenge.",
        ),
        (
            "Postgresq tunnel, CQLite profile DB, aur Air not onboarding reset teenon ko ek concise operator note mein convert kar do bhai.",
            "Postgres tunnel, SQLite profile DB, aur AirNote onboarding reset teenon ko ek concise operator note mein convert kar do bhai.",
        ),
        (
            "Sentri mein jo Grok timeout aa raha hai usko Deep gram STT aur Deep seekh polish latency ke context mein compare karna.",
            "Sentry mein jo Groq timeout aa raha hai usko Deepgram STT aur DeepSeek polish latency ke context mein compare karna.",
        ),
        (
            "MACOPS user ne bola AMEAC app mein queue drant profile prompt nahi aa raha, please CQLite cache aur Deep infra env dono verify karo.",
            "Macobs user ne bola EMIAC app mein Qdrant profile prompt nahi aa raha, please SQLite cache aur DeepInfra env dono verify karo.",
        ),
        (
            "Kaafqa consumer lag aur Zoo keep her session issue ko business impact ke saath explain karo, phir Century aur Post grass links add kar dena.",
            "Kafka consumer lag aur ZooKeeper session issue ko business impact ke saath explain karo, phir Sentry aur Postgres links add kar dena.",
        ),
        (
            "A note beta mein Sara bras model selected hai but Deep Graham fallback trigger ho raha, Grok aur Deep sick dono ka final verdict likhna.",
            "AirNote beta mein Cerebras model selected hai but Deepgram fallback trigger ho raha, Groq aur DeepSeek dono ka final verdict likhna.",
        ),
    ];
    pairs
        .iter()
        .take(target_count)
        .enumerate()
        .map(|(idx, (ai, kept))| EvalCase {
            id: format!("mixed-multiswap-{idx:02}"),
            source: "mixed_hinglish_multiswap".to_string(),
            description: "English + Hinglish multi-word/multi-swap synthetic garble".to_string(),
            raw_transcript: (*ai).to_string(),
            airnote_output: (*ai).to_string(),
            user_kept: (*kept).to_string(),
        })
        .collect()
}

fn is_usable_case(case: &EvalCase) -> bool {
    let kept = case.user_kept.trim();
    let ai = case.airnote_output.trim();
    if kept == ai || kept.len() < 8 || ai.len() < 2 {
        return false;
    }
    if kept.chars().count() > 900 || ai.chars().count() > 900 {
        return false;
    }
    if kept.split_whitespace().count() > 150 || ai.split_whitespace().count() > 150 {
        return false;
    }
    let haystack = format!("{kept}\n{ai}\n{}", case.raw_transcript).to_ascii_lowercase();
    ![
        "[notch] unknown message type",
        "thread 'main'",
        "panic",
        "sqlx::",
        "db.statement",
        "rows_affected",
        "create table if not exists",
        "relation \"",
        "already exists, skipping",
        "slow statement",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn print_case(report: &CaseReport) {
    let marker = if report.passed { "PASS" } else { "FAIL" };
    println!(
        "[{marker}] {} {}ms candidates={} approved={} aliases={} probes={}/{}",
        report.id,
        report.classify_latency_ms,
        report.review_candidates.len() + report.changes.len(),
        report.approved_items.len(),
        report.stored_aliases.len(),
        report.alias_probes.iter().filter(|p| p.matched).count(),
        report.alias_probes.len(),
    );
    if !report.profile_checks.is_empty() {
        println!(
            "  profile: v{} {} chars checks={}/{}",
            report.profile_version.unwrap_or_default(),
            report.profile_chars.unwrap_or_default(),
            report.profile_checks.iter().filter(|c| c.matched).count(),
            report.profile_checks.len(),
        );
    }
    if let Some(err) = &report.error {
        println!("  error: {err}");
    }
    if !report.approved_items.is_empty() {
        println!(
            "  approved: {}",
            report
                .approved_items
                .iter()
                .map(|i| format!("{:?}->{:?}", i.original, i.corrected))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn render_markdown(report: &EvalReport) -> String {
    let mut out = String::new();
    out.push_str("# AirNote Learning Intake Grand Test Report\n\n");
    out.push_str(&format!(
        "- Source DB: `{}`\n- Eval DB: `{}`\n- Cases: {} passed / {} total\n- Stored terms: {}\n- Stored aliases: {}\n\n",
        report.source_db.display(),
        report.eval_db.display(),
        report.passed_cases,
        report.total_cases,
        report.stored_term_total,
        report.stored_alias_total,
    ));
    out.push_str("## Case Results\n\n");
    for case in &report.cases {
        out.push_str(&format!(
            "### {} {}\n\n",
            if case.passed { "PASS" } else { "FAIL" },
            case.id
        ));
        out.push_str(&format!(
            "- Source: `{}`\n- Class: `{}`\n- Candidates: `{}`\n- Approved: `{}`\n- Aliases now: `{}`\n- Alias probes: `{}/{}`\n",
            case.source,
            case.class_name.as_deref().unwrap_or("-"),
            case.changes.len() + case.review_candidates.len(),
            case.approved_items.len(),
            case.stored_aliases.len(),
            case.alias_probes.iter().filter(|p| p.matched).count(),
            case.alias_probes.len(),
        ));
        if !case.profile_checks.is_empty() {
            out.push_str(&format!(
                "- Profile: `v{}` `{}` chars, checks `{}/{}`\n",
                case.profile_version.unwrap_or_default(),
                case.profile_chars.unwrap_or_default(),
                case.profile_checks.iter().filter(|c| c.matched).count(),
                case.profile_checks.len(),
            ));
        }
        if let Some(reason) = &case.classify_reason {
            out.push_str(&format!("- Reason: {}\n", compact(reason, 220)));
        }
        if let Some(err) = &case.error {
            out.push_str(&format!("- Error: `{}`\n", err));
        }
        if !case.approved_items.is_empty() {
            out.push_str("\nApproved items:\n\n");
            for item in &case.approved_items {
                out.push_str(&format!("- `{}` -> `{}`\n", item.original, item.corrected));
            }
        }
        let failed_probes = case
            .alias_probes
            .iter()
            .filter(|p| !p.matched)
            .collect::<Vec<_>>();
        if !failed_probes.is_empty() {
            out.push_str("\nFailed alias probes:\n\n");
            for probe in failed_probes.iter().take(10) {
                out.push_str(&format!(
                    "- expected `{}` in `{}` -> `{}`\n",
                    probe.expected, probe.input, probe.output
                ));
            }
        }
        let failed_profile_checks = case
            .profile_checks
            .iter()
            .filter(|check| !check.matched)
            .collect::<Vec<_>>();
        if !failed_profile_checks.is_empty() {
            out.push_str("\nFailed profile checks:\n\n");
            for check in failed_profile_checks {
                out.push_str(&format!("- `{}` missing `{}`\n", check.kind, check.needle));
            }
        }
        out.push('\n');
    }
    out
}

fn default_source_db() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("Library/Application Support/VoicePolish/db.sqlite")
}

fn eval_db_path() -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(".context/learning-intake-grand");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("eval-{}.sqlite", uuid::Uuid::new_v4())))
}

fn safe_id(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn norm(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn estimate_seconds(text: &str) -> f64 {
    (text.split_whitespace().count() as f64 / 2.6).clamp(1.0, 60.0)
}

fn compact(text: &str, max: usize) -> String {
    let mut out = text.replace('\n', " ");
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect::<String>();
        out.push('…');
    }
    out
}
