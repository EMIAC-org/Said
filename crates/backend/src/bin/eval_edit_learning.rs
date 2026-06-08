//! Real `/v1/classify-edit` evaluator for learning/edit interpretation.
//!
//! This is intentionally not a unit test. It starts the real backend router on
//! a temporary SQLite database, seeds recordings/vocabulary, posts real
//! classify-edit requests, and lets the route call Groq when the complex edit
//! interpreter is needed.
//!
//! Usage:
//!   GROQ_API_KEY=gsk_... cargo run -p said-backend --bin eval-edit-learning
//!   GATEWAY_API_KEY=gsk_... cargo run -p said-backend --bin eval-edit-learning -- --case skipped-macobs

use axum::Router;
use clap::Parser;
use reqwest::StatusCode;
use said_backend::{
    AppState,
    store::{
        self,
        history::{self, InsertRecording},
        vocabulary,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const SECRET: &str = "eval-edit-learning-secret";
const USER_ID_FALLBACK: &str = "eval-user";

#[derive(Parser, Debug)]
#[command(
    name = "eval-edit-learning",
    about = "Call the real /v1/classify-edit pipeline with LLM-backed complex edit cases"
)]
struct Args {
    /// Case id to run, or "all".
    #[arg(long, default_value = "all")]
    case: String,

    /// Keep the temporary SQLite DB after the run.
    #[arg(long)]
    keep_db: bool,

    /// Stop on the first failing case.
    #[arg(long)]
    fail_fast: bool,

    /// Output JSON report path.
    #[arg(long, default_value = ".context/eval-edit-learning/latest.json")]
    json_out: PathBuf,

    /// Milliseconds to wait between cases to keep Groq usage polite.
    #[arg(long, default_value_t = 2_750)]
    case_delay_ms: u64,
}

#[derive(Clone)]
struct EvalCase {
    id: &'static str,
    description: &'static str,
    raw_transcript: &'static str,
    airnote_output: &'static str,
    user_kept: &'static str,
    expected: &'static [ExpectedPair],
    forbidden: &'static [ExpectedPair],
    requires_llm: bool,
}

#[derive(Clone, Copy, Serialize)]
struct ExpectedPair {
    original: &'static str,
    corrected: &'static str,
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
    #[serde(default)]
    ambiguous_terms: Vec<AmbiguousTerm>,
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
struct AmbiguousTerm {
    original: String,
    corrected: String,
    context: String,
    recording_id: String,
}

#[derive(Serialize)]
struct CaseReport {
    id: String,
    description: String,
    requires_llm: bool,
    passed: bool,
    status: u16,
    latency_ms: u128,
    expected: Vec<ExpectedPair>,
    forbidden: Vec<ExpectedPair>,
    matched_expected: Vec<ExpectedPair>,
    matched_forbidden: Vec<ExpectedPair>,
    class_name: Option<String>,
    reason: Option<String>,
    learned: Option<bool>,
    notify: Option<bool>,
    promoted_count: Option<usize>,
    changes: Vec<Change>,
    review_candidates: Vec<ReviewCandidate>,
    ambiguous_terms: Vec<AmbiguousTerm>,
    error: Option<String>,
}

#[derive(Serialize)]
struct EvalReport {
    total: usize,
    passed: usize,
    failed: usize,
    db_path: PathBuf,
    cases: Vec<CaseReport>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    said_core::load_env();
    // This eval intentionally exercises the learning route, but it must not
    // spawn the ONNX trainer against the user's real app database.
    unsafe {
        std::env::set_var("AIRNOTE_DISABLE_ONNX_RETRAIN", "1");
    }
    let args = Args::parse();
    let groq_key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();

    if groq_key.trim().is_empty() {
        println!("SKIP: set GROQ_API_KEY or GATEWAY_API_KEY to run real LLM edit-learning evals");
        return Ok(());
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "said_backend::routes::classify=info".to_string()),
        )
        .try_init();

    let db_path = eval_db_path()?;
    let pool = store::open(&db_path);
    let user_id = store::ensure_default_user(&pool);
    seed_vocab(&pool, &user_id);

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

    let base_url = spawn_eval_server(said_backend::router_with_state(state)).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    let selected_cases = select_cases(&args.case);
    if selected_cases.is_empty() {
        anyhow::bail!("no eval case matched {:?}", args.case);
    }

    println!("DB: {}", db_path.display());
    println!("Backend: {base_url}");
    println!("Running {} edit-learning case(s)\n", selected_cases.len());

    let mut reports = Vec::new();
    for (idx, case) in selected_cases.iter().enumerate() {
        let report = run_case(&client, &base_url, &pool, &user_id, case).await;
        print_case_report(&report);
        let failed = !report.passed;
        reports.push(report);

        if failed && args.fail_fast {
            break;
        }
        if idx + 1 < selected_cases.len() {
            tokio::time::sleep(Duration::from_millis(args.case_delay_ms)).await;
        }
    }

    let passed = reports.iter().filter(|case| case.passed).count();
    let failed = reports.len().saturating_sub(passed);
    let report = EvalReport {
        total: reports.len(),
        passed,
        failed,
        db_path: db_path.clone(),
        cases: reports,
    };

    if let Some(parent) = args.json_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.json_out, serde_json::to_string_pretty(&report)?)?;
    println!(
        "\nSummary: {passed}/{} passed. JSON report: {}",
        report.total,
        args.json_out.display()
    );

    if !args.keep_db {
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    if failed > 0 {
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
    let recording_id = format!("eval-{}-{}", case.id, uuid::Uuid::new_v4());
    let _ = history::insert_recording(
        pool,
        InsertRecording {
            id: &recording_id,
            user_id,
            transcript: case.raw_transcript,
            polished: case.airnote_output,
            word_count: case.airnote_output.split_whitespace().count() as i64,
            recording_seconds: 3.0,
            model_used: "eval-edit-learning",
            confidence: Some(0.9),
            transcribe_ms: Some(0),
            embed_ms: Some(0),
            polish_ms: Some(0),
            target_app: Some("eval-edit-learning"),
            source: "eval",
            audio_id: None,
            enriched_transcript: None,
            raw_transcript: Some(case.raw_transcript),
            local_corrected_transcript: Some(case.airnote_output),
            polished_output: Some(case.airnote_output),
        },
    );

    let payload = serde_json::json!({
        "recording_id": recording_id,
        "ai_output": case.airnote_output,
        "user_kept": case.user_kept,
        "capture_method": "ax",
        "time_since_paste_ms": 12_000,
        "app_switched": false,
        "matches_clipboard": false
    });

    let started = Instant::now();
    let resp = client
        .post(format!("{base_url}/v1/classify-edit"))
        .bearer_auth(SECRET)
        .json(&payload)
        .send()
        .await;
    let latency_ms = started.elapsed().as_millis();

    let resp = match resp {
        Ok(resp) => resp,
        Err(err) => {
            return CaseReport {
                id: case.id.to_string(),
                description: case.description.to_string(),
                requires_llm: case.requires_llm,
                passed: false,
                status: 0,
                latency_ms,
                expected: case.expected.to_vec(),
                forbidden: case.forbidden.to_vec(),
                matched_expected: Vec::new(),
                matched_forbidden: Vec::new(),
                class_name: None,
                reason: None,
                learned: None,
                notify: None,
                promoted_count: None,
                changes: Vec::new(),
                review_candidates: Vec::new(),
                ambiguous_terms: Vec::new(),
                error: Some(err.to_string()),
            };
        }
    };

    let status = resp.status();
    let parsed = resp.json::<ClassifyResponse>().await;
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(err) => {
            return CaseReport {
                id: case.id.to_string(),
                description: case.description.to_string(),
                requires_llm: case.requires_llm,
                passed: false,
                status: status.as_u16(),
                latency_ms,
                expected: case.expected.to_vec(),
                forbidden: case.forbidden.to_vec(),
                matched_expected: Vec::new(),
                matched_forbidden: Vec::new(),
                class_name: None,
                reason: None,
                learned: None,
                notify: None,
                promoted_count: None,
                changes: Vec::new(),
                review_candidates: Vec::new(),
                ambiguous_terms: Vec::new(),
                error: Some(format!("failed to parse classify response: {err}")),
            };
        }
    };

    let matched_expected = case
        .expected
        .iter()
        .copied()
        .filter(|pair| response_has_pair(&parsed, pair))
        .collect::<Vec<_>>();
    let matched_forbidden = case
        .forbidden
        .iter()
        .copied()
        .filter(|pair| response_has_any_pair(&parsed, pair))
        .collect::<Vec<_>>();
    let passed = status == StatusCode::OK
        && matched_expected.len() == case.expected.len()
        && matched_forbidden.is_empty();

    CaseReport {
        id: case.id.to_string(),
        description: case.description.to_string(),
        requires_llm: case.requires_llm,
        passed,
        status: status.as_u16(),
        latency_ms,
        expected: case.expected.to_vec(),
        forbidden: case.forbidden.to_vec(),
        matched_expected,
        matched_forbidden,
        class_name: Some(parsed.class_name),
        reason: Some(parsed.reason),
        learned: Some(parsed.learned),
        notify: Some(parsed.notify),
        promoted_count: Some(parsed.promoted_count),
        changes: parsed.changes,
        review_candidates: parsed.review_candidates,
        ambiguous_terms: parsed.ambiguous_terms,
        error: None,
    }
}

fn response_has_pair(resp: &ClassifyResponse, pair: &ExpectedPair) -> bool {
    resp.changes.iter().any(|change| {
        change.reason == "stt_error"
            && change.should_learn
            && pair_matches(
                &change.original,
                &change.corrected,
                pair.original,
                pair.corrected,
            )
    }) || resp.review_candidates.iter().any(|candidate| {
        candidate.learnable
            && pair_matches(
                &candidate.original,
                &candidate.corrected,
                pair.original,
                pair.corrected,
            )
    })
}

fn response_has_any_pair(resp: &ClassifyResponse, pair: &ExpectedPair) -> bool {
    resp.changes.iter().any(|change| {
        change.should_learn
            && pair_matches(
                &change.original,
                &change.corrected,
                pair.original,
                pair.corrected,
            )
    }) || resp.review_candidates.iter().any(|candidate| {
        candidate.learnable
            && pair_matches(
                &candidate.original,
                &candidate.corrected,
                pair.original,
                pair.corrected,
            )
    }) || resp.ambiguous_terms.iter().any(|candidate| {
        pair_matches(
            &candidate.original,
            &candidate.corrected,
            pair.original,
            pair.corrected,
        )
    })
}

fn pair_matches(
    actual_original: &str,
    actual_corrected: &str,
    original: &str,
    corrected: &str,
) -> bool {
    norm(actual_original) == norm(original) && norm(actual_corrected) == norm(corrected)
}

fn norm(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_case_report(report: &CaseReport) {
    let marker = if report.passed { "PASS" } else { "FAIL" };
    println!(
        "[{marker}] {} ({}ms) — {}",
        report.id, report.latency_ms, report.description
    );
    if !report.matched_expected.is_empty() {
        println!(
            "  matched expected: {}",
            report
                .matched_expected
                .iter()
                .map(|p| format!("{:?}->{:?}", p.original, p.corrected))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !report.matched_forbidden.is_empty() {
        println!(
            "  matched forbidden: {}",
            report
                .matched_forbidden
                .iter()
                .map(|p| format!("{:?}->{:?}", p.original, p.corrected))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !report.passed {
        println!("  expected: {}", format_pairs(&report.expected));
        println!("  forbidden: {}", format_pairs(&report.forbidden));
        println!(
            "  changes: {}",
            report
                .changes
                .iter()
                .map(|c| format!(
                    "{:?}->{:?} reason={} learn={} conf={:.2}",
                    c.original, c.corrected, c.reason, c.should_learn, c.confidence
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        println!(
            "  review: {}",
            report
                .review_candidates
                .iter()
                .map(|c| format!("{:?}->{:?} type={}", c.original, c.corrected, c.term_type))
                .collect::<Vec<_>>()
                .join("; ")
        );
        println!(
            "  ambiguous: {}",
            report
                .ambiguous_terms
                .iter()
                .map(|c| format!("{:?}->{:?}", c.original, c.corrected))
                .collect::<Vec<_>>()
                .join("; ")
        );
        if let Some(error) = &report.error {
            println!("  error: {error}");
        }
    }
}

fn format_pairs(pairs: &[ExpectedPair]) -> String {
    if pairs.is_empty() {
        return "-".to_string();
    }
    pairs
        .iter()
        .map(|p| format!("{:?}->{:?}", p.original, p.corrected))
        .collect::<Vec<_>>()
        .join(", ")
}

fn eval_db_path() -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(".context/eval-edit-learning");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("eval-{}.sqlite", uuid::Uuid::new_v4())))
}

fn seed_vocab(pool: &store::DbPool, user_id: &str) {
    let terms = [
        "Macobs",
        "EMIAC",
        "AirNote",
        "n8n",
        "Kubernetes",
        "GraphQL",
        "Supabase",
        "PostgreSQL",
        "OAuth",
        "Docker",
        "Vercel",
        "Cursor",
        "Perplexity",
        "Claude",
        "Urban Aura",
        "Divo",
        "Testbot",
        "HRM8",
    ];
    for term in terms {
        let _ = vocabulary::upsert_for_language_with_context(
            pool,
            user_id,
            term,
            2.0,
            "manual",
            "hinglish",
            Some("Seeded protected eval term"),
        );
    }
}

fn select_cases(requested: &str) -> Vec<EvalCase> {
    let cases = cases();
    if requested == "all" {
        return cases;
    }
    cases
        .into_iter()
        .filter(|case| case.id == requested)
        .collect()
}

fn cases() -> Vec<EvalCase> {
    vec![
        EvalCase {
            id: "skipped-macobs",
            description: "STT skipped a protected company term; user inserted it back",
            raw_transcript: "ka data bhejo",
            airnote_output: "ka data bhejo",
            user_kept: "Macobs ka data bhejo",
            expected: &[ExpectedPair {
                original: "",
                corrected: "Macobs",
            }],
            forbidden: &[],
            requires_llm: true,
        },
        EvalCase {
            id: "broad-macobs-mein",
            description: "Common source plus filler should learn only the protected term",
            raw_transcript: "mujhe data bhejo",
            airnote_output: "Mujhe data bhejo",
            user_kept: "Macobs mein data bhejo",
            expected: &[ExpectedPair {
                original: "",
                corrected: "Macobs",
            }],
            forbidden: &[
                ExpectedPair {
                    original: "Mujhe",
                    corrected: "Macobs mein",
                },
                ExpectedPair {
                    original: "Mujhe",
                    corrected: "Macobs",
                },
                ExpectedPair {
                    original: "mein",
                    corrected: "Macobs",
                },
            ],
            requires_llm: false,
        },
        EvalCase {
            id: "skipped-n8n-emiac",
            description: "Two skipped protected terms in one edited sentence",
            raw_transcript: "ka proprietary automation model hai",
            airnote_output: "ka proprietary automation model hai",
            user_kept: "n8n EMIAC ka proprietary automation model hai",
            expected: &[
                ExpectedPair {
                    original: "",
                    corrected: "n8n",
                },
                ExpectedPair {
                    original: "",
                    corrected: "EMIAC",
                },
            ],
            forbidden: &[],
            requires_llm: true,
        },
        EvalCase {
            id: "skipped-urban-aura",
            description: "Skipped multi-word protected phrase should stay one phrase, not learn filler",
            raw_transcript: "uska naam hai",
            airnote_output: "uska naam hai",
            user_kept: "uska naam Urban Aura hai",
            expected: &[ExpectedPair {
                original: "",
                corrected: "Urban Aura",
            }],
            forbidden: &[ExpectedPair {
                original: "",
                corrected: "Urban Aura hai",
            }],
            requires_llm: false,
        },
        EvalCase {
            id: "n10-to-n8n",
            description: "Actual user-side n10 edit should identify n8n and nothing stale",
            raw_transcript: "n10 aur kafka ka use karke automation seekhni hai",
            airnote_output: "n10 aur Kafka ka use karke automation seekhni hai",
            user_kept: "n8n aur Kafka ka use karke automation seekhni hai",
            expected: &[ExpectedPair {
                original: "n10",
                corrected: "n8n",
            }],
            forbidden: &[
                ExpectedPair {
                    original: "karke",
                    corrected: "Groq",
                },
                ExpectedPair {
                    original: "use",
                    corrected: "Groq",
                },
            ],
            requires_llm: false,
        },
        EvalCase {
            id: "large-hunk-dev-terms",
            description: "Large hunk contains several local dev term corrections",
            raw_transcript: "graph cute super base and post grey sequel ka auth flow cursor mein debug karna",
            airnote_output: "graph cute super base and post grey sequel ka auth flow cursor mein debug karna",
            user_kept: "GraphQL Supabase and PostgreSQL ka OAuth flow Cursor mein debug karna",
            expected: &[
                ExpectedPair {
                    original: "graph cute",
                    corrected: "GraphQL",
                },
                ExpectedPair {
                    original: "super base",
                    corrected: "Supabase",
                },
                ExpectedPair {
                    original: "post grey sequel",
                    corrected: "PostgreSQL",
                },
                ExpectedPair {
                    original: "auth",
                    corrected: "OAuth",
                },
            ],
            forbidden: &[],
            requires_llm: true,
        },
        EvalCase {
            id: "kubernetes-cluster-complex",
            description: "Ugly Kubernetes distortion plus multi-word context edit",
            raw_transcript: "cube net ease cluster mein graph cute resolver ka issue aa raha hai",
            airnote_output: "cube net ease cluster mein graph cute resolver ka issue aa raha hai",
            user_kept: "Kubernetes cluster mein GraphQL resolver ka issue aa raha hai",
            expected: &[
                ExpectedPair {
                    original: "cube net ease",
                    corrected: "Kubernetes",
                },
                ExpectedPair {
                    original: "graph cute",
                    corrected: "GraphQL",
                },
            ],
            forbidden: &[],
            requires_llm: true,
        },
        EvalCase {
            id: "brand-list-complex",
            description: "Distorted brand/tool list with several protected replacements",
            raw_transcript: "cloud and per plex city ke saath docker aur worker sell use karna",
            airnote_output: "cloud and per plex city ke saath docker aur worker sell use karna",
            user_kept: "Claude and Perplexity ke saath Docker aur Vercel use karna",
            expected: &[
                ExpectedPair {
                    original: "cloud",
                    corrected: "Claude",
                },
                ExpectedPair {
                    original: "per plex city",
                    corrected: "Perplexity",
                },
                ExpectedPair {
                    original: "worker sell",
                    corrected: "Vercel",
                },
            ],
            forbidden: &[],
            requires_llm: true,
        },
        EvalCase {
            id: "common-hindi-negative",
            description: "Common Hindi correction must not become Macobs learning",
            raw_transcript: "ye kaisa laga",
            airnote_output: "ye Macobs laga",
            user_kept: "ye kaisa laga",
            expected: &[],
            forbidden: &[ExpectedPair {
                original: "kaisa",
                corrected: "Macobs",
            }],
            requires_llm: false,
        },
        EvalCase {
            id: "style-rewrite-negative",
            description: "Polished business rewrite should not invent local vocabulary pairs",
            raw_transcript: "please make this message better for client",
            airnote_output: "Please make this message better for the client.",
            user_kept: "Please make this message more professional and concise for the client.",
            expected: &[],
            forbidden: &[
                ExpectedPair {
                    original: "professional",
                    corrected: "Macobs",
                },
                ExpectedPair {
                    original: "client",
                    corrected: "EMIAC",
                },
            ],
            requires_llm: true,
        },
        EvalCase {
            id: "real-word-negative",
            description: "Real dictionary word collision should not auto-learn a protected term",
            raw_transcript: "cursor word ka normal english meaning batao",
            airnote_output: "cursor word ka normal English meaning batao",
            user_kept: "curser word ka normal English meaning batao",
            expected: &[],
            forbidden: &[ExpectedPair {
                original: "cursor",
                corrected: "Cursor",
            }],
            requires_llm: false,
        },
    ]
}
