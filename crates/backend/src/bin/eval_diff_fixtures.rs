//! Synthetic STT→expected fixture harness for the edit-diff / classify pipeline.
//!
//! Unlike `eval_learning_intake` (which replays *real* edit history), this feeds
//! hand-built `{transcript, ai_output, pre_existing_text, edit}` fixtures through
//! the **real** `/v1/classify-edit` route on a temp DB and checks the correction
//! candidates the deterministic diff produces. It locks down the "diff viewer"
//! edge cases: swaps when the field already has text (focus only on our output),
//! line changes, whole-sentence + high-count replacements, inserts, deletes,
//! mixed edits, no-ops, unicode/Devanagari/emoji, and adversarial scoping.
//!
//! The field is assembled exactly like production:
//!     user_kept = prior_prefix + edited_output + prior_suffix
//! and `prior_text` (= the pre-dictation baseline) is sent alongside so the
//! pipeline can scope the diff to our own output.
//!
//! Run:  cargo run -p said-backend --bin eval_diff_fixtures
//! Exit code is non-zero if any fixture fails (CI-friendly).

use axum::Router;
use said_backend::{
    AppState,
    store::{
        self,
        history::{self, InsertRecording},
    },
};
use serde_json::Value;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::RwLock;

const SECRET: &str = "diff-fixtures-secret";

/// One synthetic edit case. The field is built as
/// `prior_prefix + ai_output(with edits applied) + prior_suffix`.
#[derive(Default)]
struct Fixture {
    id: &'static str,
    transcript: &'static str,
    ai_output: &'static str,
    /// Text already in the field BEFORE our paste, split at the caret.
    prior_prefix: &'static str,
    prior_suffix: &'static str,
    /// Override the baseline we *send* (to simulate the user also editing the
    /// surrounding text, so the baseline no longer matches the field).
    prior_text_override: Option<&'static str>,
    /// Literal (old, new) substring replacements applied to `ai_output`.
    edits: &'static [(&'static str, &'static str)],
    /// Pairs that MUST appear among the candidates (original, corrected).
    expect_pairs: &'static [(&'static str, &'static str)],
    /// Surfaces that must NOT appear as a candidate's `corrected`.
    forbid_corrected: &'static [&'static str],
    /// Candidate list (deduped) must be empty.
    expect_no_candidates: bool,
    /// No candidate may have an empty original (forces the swap-only check even
    /// when `expect_pairs` is empty; default derives from expect_pairs).
    forbid_empty_original: bool,
    /// Opt out of the empty-original check (documented graceful-degradation
    /// cases: deliberate baseline mismatch, mixed swap+insert protected terms).
    allow_empty_original: bool,
    /// Cap on deduped candidate count (0 = unlimited). Keeps big rewrites graceful.
    max_candidates: usize,
    /// No candidate (original or corrected) may contain Devanagari.
    forbid_devanagari: bool,
}

fn apply_edits(ai_output: &str, edits: &[(&str, &str)]) -> String {
    let mut s = ai_output.to_string();
    for (old, new) in edits {
        s = s.replacen(old, new, 1);
    }
    s
}

#[rustfmt::skip]
fn fixtures() -> Vec<Fixture> {
    vec![
        // ── A. Basic swaps, empty field ─────────────────────────────────────
        Fixture { id: "A1-swap-basic", transcript: "isko jaldi se dekh lo aur max kar do",
            ai_output: "Isko jaldi se dekh lo aur max kar do.",
            edits: &[("max", "EMIAC")], expect_pairs: &[("max", "EMIAC")], ..Default::default() },
        Fixture { id: "A2-swap-vocab-n8n", transcript: "i use written for automation",
            ai_output: "I use written for automation.",
            edits: &[("written", "n8n")], expect_pairs: &[("written", "n8n")], ..Default::default() },
        // Pure case-only changes are intentionally NOT learned (clean_surface is
        // case-insensitive); assert they stay graceful (no garbage candidate).
        Fixture { id: "A3-case-only-react", transcript: "deploy to react today",
            ai_output: "Deploy to react today.",
            edits: &[("react", "React")], expect_no_candidates: true, ..Default::default() },
        Fixture { id: "A4-case-only-kafka", transcript: "we use kafka",
            ai_output: "We use kafka.",
            edits: &[("kafka.", "Kafka.")], expect_no_candidates: true, ..Default::default() },
        Fixture { id: "A5-case-only-api", transcript: "ping the api now",
            ai_output: "Ping the api now.",
            edits: &[("api", "API")], expect_no_candidates: true, ..Default::default() },

        // ── B. Swaps with pre-existing text (scoping) ───────────────────────
        Fixture { id: "B1-prefix-text", transcript: "isko dekh lo aur max kar do",
            ai_output: "Isko dekh lo aur max kar do.", prior_prefix: "Hello team, quick note. ",
            edits: &[("max", "EMIAC")], expect_pairs: &[("max", "EMIAC")],
            forbid_corrected: &["Hello", "team,", "note."], ..Default::default() },
        Fixture { id: "B2-suffix-text", transcript: "isko dekh lo aur max kar do",
            ai_output: "Isko dekh lo aur max kar do.", prior_suffix: " Thanks and regards, Abhishek.",
            edits: &[("max", "EMIAC")], expect_pairs: &[("max", "EMIAC")],
            forbid_corrected: &["Thanks", "regards,", "Abhishek."], ..Default::default() },
        Fixture { id: "B3-both-sides", transcript: "isko dekh lo aur max kar do",
            ai_output: "Isko dekh lo aur max kar do.", prior_prefix: "Subject: status. ",
            prior_suffix: " Sent from my phone.", edits: &[("max", "EMIAC")],
            expect_pairs: &[("max", "EMIAC")], forbid_corrected: &["Subject:", "Sent", "phone."],
            ..Default::default() },
        Fixture { id: "B4-multiline", transcript: "deploy the max build to staging",
            ai_output: "Deploy the max build to staging.",
            prior_prefix: "Line one already here\nLine two already here\n",
            prior_suffix: "\nLine four trailing", edits: &[("max", "EMIAC")],
            expect_pairs: &[("max", "EMIAC")], forbid_corrected: &["Line", "one", "four"],
            ..Default::default() },
        Fixture { id: "B5-prior-repeats-word", transcript: "set the max value now",
            ai_output: "Set the max value now.", prior_prefix: "The max allowed is fixed. ",
            edits: &[("max value", "EMIAC value")], expect_pairs: &[("max", "EMIAC")],
            forbid_corrected: &["allowed", "fixed."], ..Default::default() },
        Fixture { id: "B6-prior-equals-output", transcript: "set the max value now",
            ai_output: "Set the max value now.", prior_prefix: "Set the max value now. ",
            edits: &[("max value", "EMIAC value")], expect_pairs: &[("max", "EMIAC")],
            forbid_empty_original: true, ..Default::default() },
        Fixture { id: "B7-prior-word-twice", transcript: "set the max value now",
            ai_output: "Set the max value now.", prior_prefix: "max here and max there. ",
            edits: &[("max value", "EMIAC value")], expect_pairs: &[("max", "EMIAC")],
            forbid_corrected: &["here", "there"], ..Default::default() },
        // Baseline no longer matches the field (user also edited the surrounding
        // text) → scoping gracefully falls back to the full field. The swap is
        // still found; some pre-existing text may surface (documented limit).
        Fixture { id: "B8-baseline-mismatch-fallback", transcript: "set the max value now",
            ai_output: "Set the max value now.", prior_prefix: "Fixed typo here. ",
            prior_text_override: Some("Different earlier text. "),
            edits: &[("max", "EMIAC")], expect_pairs: &[("max", "EMIAC")],
            allow_empty_original: true, ..Default::default() },

        // ── C. High replacement counts ──────────────────────────────────────
        Fixture { id: "C1-high-count-3", transcript: "the quick brown fox",
            ai_output: "the quick brown fox",
            edits: &[("quick", "slow"), ("brown", "red"), ("fox", "cat")],
            expect_pairs: &[("quick", "slow"), ("brown", "red"), ("fox", "cat")], ..Default::default() },
        Fixture { id: "C2-high-count-5", transcript: "the quick brown fox jumps high",
            ai_output: "the quick brown fox jumps high",
            edits: &[("quick","slow"),("brown","red"),("fox","cat"),("jumps","runs"),("high","low")],
            expect_pairs: &[("quick","slow"),("brown","red"),("fox","cat"),("jumps","runs"),("high","low")],
            ..Default::default() },
        Fixture { id: "C3-high-count-8", transcript: "alpha bravo charlie delta echo foxtrot golf hotel",
            ai_output: "alpha bravo charlie delta echo foxtrot golf hotel",
            edits: &[("alpha","a1"),("bravo","b2"),("charlie","c3"),("delta","d4"),
                ("echo","e5"),("foxtrot","f6"),("golf","g7"),("hotel","h8")],
            expect_pairs: &[("alpha","a1"),("delta","d4"),("hotel","h8")], ..Default::default() },
        Fixture { id: "C4-non-contiguous-2swaps", transcript: "the cat and the dog ran",
            ai_output: "The cat and the dog ran.",
            edits: &[("cat", "fox"), ("dog", "wolf")],
            expect_pairs: &[("cat", "fox"), ("dog", "wolf")], ..Default::default() },
        Fixture { id: "C5-high-count-with-prefix", transcript: "the quick brown fox",
            ai_output: "the quick brown fox", prior_prefix: "Earlier context line here. ",
            edits: &[("quick", "slow"), ("brown", "red"), ("fox", "cat")],
            expect_pairs: &[("quick", "slow"), ("brown", "red"), ("fox", "cat")],
            forbid_corrected: &["Earlier", "context"], ..Default::default() },

        // ── D. Whole-sentence / phrase rewrites (graceful) ──────────────────
        Fixture { id: "D1-whole-sentence", transcript: "let us schedule the meeting for tomorrow morning",
            ai_output: "Let us schedule the meeting for tomorrow morning.", prior_prefix: "Note: ",
            edits: &[("Let us schedule the meeting for tomorrow morning.",
                "Cancel everything and call me right now.")],
            forbid_corrected: &["Note:"], max_candidates: 2, ..Default::default() },
        Fixture { id: "D2-phrase-shrink", transcript: "let us schedule the meeting now",
            ai_output: "Let us schedule the meeting now.",
            edits: &[("Let us schedule the meeting now.", "Cancel it.")],
            max_candidates: 2, ..Default::default() },
        Fixture { id: "D3-phrase-delete-and-swap", transcript: "the very big report draft",
            ai_output: "The very big report draft.",
            edits: &[("very big", "huge")], max_candidates: 3, ..Default::default() },

        // ── E. Insertions ───────────────────────────────────────────────────
        Fixture { id: "E1-insert-middle", transcript: "send the report today",
            ai_output: "Send the report today.", prior_prefix: "FYI. ",
            edits: &[("report today", "final report today")],
            forbid_corrected: &["FYI."], max_candidates: 2, ..Default::default() },
        Fixture { id: "E2-insert-start", transcript: "report ready",
            ai_output: "Report ready.", edits: &[("Report ready", "The report ready")],
            max_candidates: 2, ..Default::default() },
        Fixture { id: "E3-insert-end", transcript: "all done",
            ai_output: "All done.", edits: &[("done.", "done now.")],
            max_candidates: 2, ..Default::default() },
        Fixture { id: "E4-insert-vocab-term", transcript: "use it daily for automation",
            ai_output: "Use it daily for automation.", edits: &[("use it", "use n8n")],
            forbid_devanagari: true, max_candidates: 3, ..Default::default() },
        Fixture { id: "E5-insert-email-markdown", transcript: "Anish at Gmail dot com ka zara batana",
            ai_output: "Anish at Gmail dot com ka zara batana",
            edits: &[("Anish at", "[anish@gmail.com](mailto:anish@gmail.com) Anish at")],
            forbid_devanagari: true, ..Default::default() },

        // ── F. Deletions ────────────────────────────────────────────────────
        Fixture { id: "F1-delete-word", transcript: "send the big report today",
            ai_output: "Send the big report today.", prior_prefix: "FYI. ",
            edits: &[("big ", "")], forbid_corrected: &["FYI."], max_candidates: 2,
            ..Default::default() },
        Fixture { id: "F2-delete-phrase", transcript: "please kindly send it over",
            ai_output: "Please kindly send it over.", edits: &[("kindly ", "")],
            max_candidates: 2, ..Default::default() },
        Fixture { id: "F3-delete-filler", transcript: "um so basically send it",
            ai_output: "Um, so basically send it.", edits: &[("Um, so basically ", "")],
            max_candidates: 3, ..Default::default() },

        // ── G. Mixed edits ──────────────────────────────────────────────────
        // Mixed edits: the contiguous insert/delete bundles with the swap. The
        // swap source ("max" / "big max") is still identified; a protected-term
        // insert of EMIAC may also fire (graceful).
        Fixture { id: "G1-swap-plus-insert", transcript: "use max here please",
            ai_output: "Use max here please.", edits: &[("max here", "EMIAC value here")],
            expect_pairs: &[("max", "EMIAC value")], allow_empty_original: true,
            ..Default::default() },
        Fixture { id: "G2-swap-plus-delete", transcript: "the big max value here",
            ai_output: "The big max value here.", edits: &[("big max", "EMIAC")],
            expect_pairs: &[("big max", "EMIAC")], ..Default::default() },

        // ── H. No-ops / trivial (nothing learnable) ─────────────────────────
        Fixture { id: "H1-no-edit", transcript: "send the report today",
            ai_output: "Send the report today.", expect_no_candidates: true, ..Default::default() },
        Fixture { id: "H2-whitespace-only", transcript: "send it now",
            ai_output: "Send it now.", edits: &[("Send it", "Send  it")],
            expect_no_candidates: true, ..Default::default() },
        Fixture { id: "H3-trailing-newline", transcript: "send it now",
            ai_output: "Send it now.", prior_suffix: "\n",
            expect_no_candidates: true, ..Default::default() },

        // ── I. Unicode / script / emoji ─────────────────────────────────────
        Fixture { id: "I1-emoji-adjacent", transcript: "ship it now",
            ai_output: "Ship it 🚀 now.", edits: &[("Ship", "Deploy")],
            expect_pairs: &[("ship", "deploy")], forbid_devanagari: true, ..Default::default() },
        Fixture { id: "I2-hinglish-context", transcript: "namaste max bhai kaise ho",
            ai_output: "Namaste max bhai, kaise ho.", edits: &[("max", "EMIAC")],
            expect_pairs: &[("max", "EMIAC")], forbid_devanagari: true, ..Default::default() },
        Fixture { id: "I3-accented-corrected", transcript: "the cafe is open",
            ai_output: "The cafe is open.", edits: &[("cafe", "café")],
            expect_pairs: &[("cafe", "café")], ..Default::default() },

        // ── J. Adversarial / large ──────────────────────────────────────────
        Fixture { id: "J1-long-output-deep-swap",
            transcript: "okay so today the plan is to first review the max settings then deploy the build and finally update the docs before the team standup tomorrow morning",
            ai_output: "Okay so today the plan is to first review the max settings then deploy the build and finally update the docs before the team standup tomorrow morning.",
            prior_prefix: "Standup notes. ", edits: &[("max", "EMIAC")],
            expect_pairs: &[("max", "EMIAC")], forbid_corrected: &["Standup", "notes."],
            ..Default::default() },
        Fixture { id: "J2-internal-newline-swap", transcript: "first line second has max value",
            ai_output: "First line.\nSecond has max value.", edits: &[("max", "EMIAC")],
            expect_pairs: &[("max", "EMIAC")], ..Default::default() },
        Fixture { id: "J3-punctuation-only", transcript: "are you sure",
            ai_output: "Are you sure.", edits: &[("sure.", "sure?")],
            max_candidates: 1, ..Default::default() },
    ]
}

fn temp_db() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "diff_fixtures_{}_{}.sqlite",
        std::process::id(),
        nanos
    ))
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

/// Pull every (original, corrected) candidate the response surfaces, across all
/// the buckets the desktop card reads from, deduped.
fn extract_candidates(v: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for key in ["changes", "review_candidates", "ambiguous_terms"] {
        if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
            for item in arr {
                let o = item
                    .get("original")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let c = item
                    .get("corrected")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if o.is_empty() && c.is_empty() {
                    continue;
                }
                let key = (o.trim().to_lowercase(), c.trim().to_lowercase());
                if seen.insert(key) {
                    out.push((o, c));
                }
            }
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn has_devanagari(s: &str) -> bool {
    s.chars().any(|c| ('\u{0900}'..'\u{0980}').contains(&c))
}

#[tokio::main]
async fn main() {
    said_core::load_env();
    unsafe {
        std::env::set_var("AIRNOTE_DISABLE_ONNX_RETRAIN", "1");
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .try_init();

    let db = temp_db();
    let pool = store::open(&db);
    let user_id = store::ensure_default_user(&pool);

    let state = AppState {
        pool: pool.clone(),
        shared_secret: Arc::new(SECRET.to_string()),
        default_user_id: Arc::new(user_id.clone()),
        prefs_cache: Arc::new(RwLock::new(None)),
        lexicon_cache: Arc::new(RwLock::new(None)),
        live_server_runtime_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
        http_client: reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .unwrap(),
        watchdog: Arc::new(said_backend::watchdog::WatchdogState::new()),
    };

    let base = spawn(said_backend::router_with_state(state)).await;
    let client = reqwest::Client::new();

    let cases = fixtures();
    let total = cases.len();
    let mut passed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for fx in &cases {
        let edited = apply_edits(fx.ai_output, fx.edits);
        let prior_text = fx
            .prior_text_override
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}{}", fx.prior_prefix, fx.prior_suffix));
        let user_kept = format!("{}{}{}", fx.prior_prefix, edited, fx.prior_suffix);
        let recording_id = format!("fx-{}-{}", fx.id, std::process::id());

        let _ = history::insert_recording(
            &pool,
            InsertRecording {
                id: &recording_id,
                user_id: &user_id,
                transcript: fx.transcript,
                polished: fx.ai_output,
                word_count: fx.ai_output.split_whitespace().count() as i64,
                recording_seconds: 2.0,
                model_used: "eval-diff-fixtures",
                confidence: Some(0.95),
                transcribe_ms: Some(0),
                embed_ms: Some(0),
                polish_ms: Some(0),
                target_app: Some("eval-diff-fixtures"),
                source: "eval",
                audio_id: None,
                enriched_transcript: None,
                raw_transcript: Some(fx.transcript),
                local_corrected_transcript: Some(fx.ai_output),
                polished_output: Some(fx.ai_output),
            },
        );

        let payload = serde_json::json!({
            "recording_id": recording_id,
            "ai_output": fx.ai_output,
            "user_kept": user_kept,
            "prior_text": prior_text,
            "capture_method": "ax",
            "time_since_paste_ms": 12_000,
            "app_switched": false,
            "matches_clipboard": false,
            "client_run_id": fx.id,
        });

        let resp = client
            .post(format!("{base}/v1/classify-edit"))
            .bearer_auth(SECRET)
            .json(&payload)
            .send()
            .await
            .expect("classify request");
        let status = resp.status();
        let body: Value = resp.json().await.expect("classify json");
        let cands = extract_candidates(&body);

        let mut problems: Vec<String> = Vec::new();
        if !status.is_success() {
            problems.push(format!("HTTP {status}"));
        }
        for (o, c) in fx.expect_pairs {
            if !cands
                .iter()
                .any(|(co, cc)| norm(co) == norm(o) && norm(cc) == norm(c))
            {
                problems.push(format!("missing pair ({o}→{c})"));
            }
        }
        // Swap cases must never carry an empty original (the "was —" bug).
        if (!fx.expect_pairs.is_empty() || fx.forbid_empty_original) && !fx.allow_empty_original {
            for (o, c) in &cands {
                if o.trim().is_empty() && !c.trim().is_empty() {
                    problems.push(format!("empty-original candidate: was \"—\" → {c}"));
                }
            }
        }
        for bad in fx.forbid_corrected {
            if cands.iter().any(|(_, c)| norm(c) == norm(bad)) {
                problems.push(format!("pre-existing leaked: {bad}"));
            }
        }
        if fx.expect_no_candidates && !cands.is_empty() {
            problems.push(format!("expected no candidates, got {}", cands.len()));
        }
        if fx.max_candidates > 0 && cands.len() > fx.max_candidates {
            problems.push(format!(
                "too many candidates: {} > {}",
                cands.len(),
                fx.max_candidates
            ));
        }
        if fx.forbid_devanagari
            && cands
                .iter()
                .any(|(o, c)| has_devanagari(o) || has_devanagari(c))
        {
            problems.push("Devanagari fabricated in candidate".to_string());
        }

        if problems.is_empty() {
            passed += 1;
            println!("✅ {:<28} {:?}", fx.id, cands);
        } else {
            let line = format!("❌ {:<28} {} | {:?}", fx.id, problems.join("; "), cands);
            println!("{line}");
            failures.push(line);
        }
    }

    let _ = std::fs::remove_file(&db);
    println!("\n{passed}/{total} fixtures passed");
    if !failures.is_empty() {
        println!("\nFailures:");
        for f in &failures {
            println!("  {f}");
        }
        std::process::exit(1);
    }
}
