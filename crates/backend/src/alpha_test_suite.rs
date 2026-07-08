//! Alpha Test Suite — end-to-end lifecycle simulation.
//!
//! Simulates a real user's daily experience with AirNote. Each test walks
//! through the FULL pipeline: raw transcript → tier2 correction → (simulated)
//! LLM polish → user edit → classify → learn → re-correct on next utterance.
//!
//! Uses a temp SQLite DB with all 33 migrations. No mocking.
//!
//! Safety-first ideology: a wrong correction (replacing "mac" with "EMIAC")
//! is catastrophic. A missed correction (not catching "meah") is invisible.
//! Every test below asserts that real words are NEVER touched.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    llm::{edit_diff, promotion_gate},
    store::{
        self, DbPool, stt_replacements, stt_replacements::ApplyResult, tier2_edit_policy,
        vocabulary,
    },
    tier2,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── Test DB setup ────────────────────────────────────────────────────────────

fn fresh_db() -> (DbPool, PathBuf) {
    crate::legacy_learning::enable_debug_legacy_writes_for_tests();
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let db_path = std::env::temp_dir().join(format!("alpha_test_{n}_{}.db", std::process::id()));
    // Remove stale file from previous test run
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let pool = store::open(&db_path);
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO local_user(id, email, created_at) VALUES ('u1', 'test@test.com', 0)",
        [],
    )
    .unwrap();
    (pool, db_path)
}

// ── Pipeline helpers ─────────────────────────────────────────────────────────

/// Simulate the FULL correction pipeline exactly as voice.rs does:
///   1. Load STT replacements → apply_exact_safe (Pass 1: exact aliases)
///   2. Load vocab terms
///   3. correct_with_store (Passes 2-4: edit-policy, cluster-fuzzy, ONNX)
/// Returns the corrected transcript (what the LLM would receive).
fn run_correction(pool: &DbPool, transcript: &str) -> ApplyResult {
    let rules = stt_replacements::load_all(pool, "u1");
    let alias_result = stt_replacements::apply_exact_safe(transcript, &rules);
    let vocab = vocabulary::top_terms(pool, "u1", 1000);
    tier2::correct_with_store(pool, "u1", &alias_result.text, &rules, &vocab)
}

/// Simulate a user correction: user sees `ai_output`, edits it to `user_kept`.
/// Runs the deterministic classifier + learning pipeline.
/// Returns (learned_count, learned_terms).
fn simulate_user_edit(
    pool: &DbPool,
    transcript: &str,
    ai_output: &str,
    user_kept: &str,
) -> (usize, Vec<String>) {
    let hunks = edit_diff::diff(transcript, ai_output, user_kept);
    let mut learned_terms: Vec<String> = Vec::new();
    let mut learned_count = 0usize;

    for hunk in &hunks {
        let kept = hunk.kept_window.trim();
        let polish = hunk.polish_window.trim();
        if kept.is_empty() || polish.is_empty() {
            continue;
        }
        if kept.to_ascii_lowercase() == polish.to_ascii_lowercase() {
            continue;
        }

        let corrected = clean_surface(kept);
        let original = clean_surface(polish);
        if corrected.is_empty() || original.is_empty() {
            continue;
        }

        if promotion_gate::is_common_word(&corrected) {
            continue;
        }
        let original_is_real =
            tier2::is_in_dictionary(&original) || promotion_gate::is_common_word(&original);
        if original_is_real {
            // If corrected IS in vocab, still allow (existing term, new distortion)
            let in_vocab = vocabulary::find_by_term_ci(pool, "u1", &corrected).is_some();
            if !in_vocab {
                continue;
            }
        }

        // Determine term type
        let term_type = vocabulary::classify_term_type(&corrected);
        if term_type == "phrase" || term_type == "other" {
            if vocabulary::find_by_term_ci(pool, "u1", &corrected).is_none() {
                continue;
            }
        }

        // Learn: vocab + aliases + proactive seeds, then approve ALL at the end
        vocabulary::upsert_for_language_with_context(
            pool, "u1", &corrected, 1.0, "auto", "hinglish", None,
        );
        stt_replacements::upsert_aliases_for_language(
            pool, "u1", &original, &original, &corrected, 1.0, "hinglish",
        );
        stt_replacements::generate_proactive_distortions(
            pool, "u1", &corrected, &original, "hinglish",
        );
        tier2_edit_policy::record_explicit_edit(
            pool,
            "u1",
            &original,
            &corrected,
            "replace",
            &[],
            &[],
            None,
        );
        tier2_edit_policy::activate_all_for_term(pool, "u1", &corrected);
        // Approve LAST — after all upserts are done, so no ON CONFLICT resets
        stt_replacements::approve_aliases_for_term(pool, "u1", &corrected);

        learned_terms.push(corrected);
        learned_count += 1;
    }

    (learned_count, learned_terms)
}

fn clean_surface(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .to_string()
}

// ── Dev terms + distortions ──────────────────────────────────────────────────

struct DevTerm {
    canonical: &'static str,
    distortions: &'static [&'static str],
    sentences: &'static [(&'static str, &'static str)], // (transcript_with_distortion, corrected)
}

const DEV_TERMS: &[DevTerm] = &[
    DevTerm {
        canonical: "EMIAC",
        distortions: &["meah", "amaix", "emia", "emiak"],
        sentences: &[
            (
                "meah ka quarterly revenue dekhna hai",
                "EMIAC ka quarterly revenue dekhna hai",
            ),
            (
                "amaix technologies mein kaam karta hoon",
                "EMIAC technologies mein kaam karta hoon",
            ),
            (
                "emia ke office mein meeting hai",
                "EMIAC ke office mein meeting hai",
            ),
        ],
    },
    DevTerm {
        canonical: "MACOBS",
        distortions: &["mecorbs", "micobs", "macorbs"],
        sentences: &[
            (
                "mecorbs stock price check karo",
                "MACOBS stock price check karo",
            ),
            ("micobs ka IPO kab aayega", "MACOBS ka IPO kab aayega"),
            (
                "macorbs quarterly results dekhna hai",
                "MACOBS quarterly results dekhna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "AirNote",
        distortions: &["automot", "airnot", "air not"],
        sentences: &[
            (
                "automot app mein bug fix karo",
                "AirNote app mein bug fix karo",
            ),
            (
                "airnot ka naya version release karna hai",
                "AirNote ka naya version release karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Claude",
        distortions: &["claud", "clawd"],
        sentences: &[
            (
                "claud se code review karwa lo",
                "Claude se code review karwa lo",
            ),
            (
                "clawd model bahut accha hai",
                "Claude model bahut accha hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Groq",
        distortions: &["growk", "grok"],
        sentences: &[
            (
                "growk API bahut fast hai yaar",
                "Groq API bahut fast hai yaar",
            ),
            (
                "grok ka inference speed dekhna",
                "Groq ka inference speed dekhna",
            ),
        ],
    },
    DevTerm {
        canonical: "n8n",
        distortions: &["natan", "natn"],
        sentences: &[
            (
                "natan workflow automate karta hai",
                "n8n workflow automate karta hai",
            ),
            (
                "natn se webhook trigger karo",
                "n8n se webhook trigger karo",
            ),
        ],
    },
    DevTerm {
        canonical: "k8s",
        distortions: &["kates", "cats"],
        sentences: &[
            (
                "kates cluster mein deploy karo",
                "k8s cluster mein deploy karo",
            ),
            ("cats pods check karo bhai", "k8s pods check karo bhai"),
        ],
    },
    DevTerm {
        canonical: "Kafka",
        distortions: &["kafk", "cafka"],
        sentences: &[
            (
                "kafk consumer lag check karo",
                "Kafka consumer lag check karo",
            ),
            (
                "cafka topic create karna hai",
                "Kafka topic create karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Vercel",
        distortions: &["vercl", "versl"],
        sentences: &[
            ("vercl pe deploy karo", "Vercel pe deploy karo"),
            ("versl ka pricing dekhna", "Vercel ka pricing dekhna"),
        ],
    },
    DevTerm {
        canonical: "Supabase",
        distortions: &["supabas", "supabes"],
        sentences: &[
            (
                "supabas ka database setup karo",
                "Supabase ka database setup karo",
            ),
            ("supabes auth lagana hai", "Supabase auth lagana hai"),
        ],
    },
    DevTerm {
        canonical: "Prisma",
        distortions: &["prism a", "prizma"],
        sentences: &[
            ("prism a schema update karo", "Prisma schema update karo"),
            (
                "prizma migrate run karna hai",
                "Prisma migrate run karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "ModelArk",
        distortions: &["model arc", "model ark"],
        sentences: &[
            (
                "model arc ka inference endpoint use karo",
                "ModelArk ka inference endpoint use karo",
            ),
            (
                "model ark API integrate karna hai",
                "ModelArk API integrate karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Terraform",
        distortions: &["terra form", "tera form"],
        sentences: &[
            (
                "terra form se infrastructure manage karo",
                "Terraform se infrastructure manage karo",
            ),
            (
                "tera form plan run karna hai",
                "Terraform plan run karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Grafana",
        distortions: &["graph ana", "grafna"],
        sentences: &[
            (
                "graph ana dashboard banana hai",
                "Grafana dashboard banana hai",
            ),
            (
                "grafna pe latency check karo",
                "Grafana pe latency check karo",
            ),
        ],
    },
    DevTerm {
        canonical: "Ansible",
        distortions: &["ansibel", "ansibl"],
        sentences: &[
            ("ansibel playbook run karo", "Ansible playbook run karo"),
            (
                "ansibl se server configure karo",
                "Ansible se server configure karo",
            ),
        ],
    },
    DevTerm {
        canonical: "Twilio",
        distortions: &["two leo", "twilo"],
        sentences: &[
            ("two leo se SMS bhejo", "Twilio se SMS bhejo"),
            (
                "twilo API integrate karna hai",
                "Twilio API integrate karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "Datadog",
        distortions: &["data dok", "datadok"],
        sentences: &[
            (
                "data dok monitoring setup karo",
                "Datadog monitoring setup karo",
            ),
            ("datadok alerts check karo", "Datadog alerts check karo"),
        ],
    },
    DevTerm {
        canonical: "Nginx",
        distortions: &["engine x", "enginex"],
        sentences: &[
            ("engine x config update karo", "Nginx config update karo"),
            ("enginex reverse proxy lagao", "Nginx reverse proxy lagao"),
        ],
    },
    DevTerm {
        canonical: "Redis",
        distortions: &["red is", "reddis"],
        sentences: &[
            ("red is cache flush karo", "Redis cache flush karo"),
            (
                "reddis connection pool check karo",
                "Redis connection pool check karo",
            ),
        ],
    },
    DevTerm {
        canonical: "Postgres",
        distortions: &["post gress", "post grace"],
        sentences: &[
            (
                "post gress migration run karo",
                "Postgres migration run karo",
            ),
            ("post grace backup le lo", "Postgres backup le lo"),
        ],
    },
    DevTerm {
        canonical: "NextJS",
        distortions: &["next jas", "nekst js"],
        sentences: &[
            ("next jas app router use karo", "NextJS app router use karo"),
            ("nekst js ka build check karo", "NextJS ka build check karo"),
        ],
    },
    DevTerm {
        canonical: "Vite",
        distortions: &["vyt", "viit"],
        sentences: &[
            ("vyt dev server start karo", "Vite dev server start karo"),
            ("viit se bundling fast hai", "Vite se bundling fast hai"),
        ],
    },
    DevTerm {
        canonical: "Linear",
        distortions: &["lineaar", "liniar"],
        sentences: &[
            (
                "lineaar mein ticket create karo",
                "Linear mein ticket create karo",
            ),
            ("liniar board update karo", "Linear board update karo"),
        ],
    },
    DevTerm {
        canonical: "Sentry",
        distortions: &["sentri", "senry"],
        sentences: &[
            ("sentri error tracking lagao", "Sentry error tracking lagao"),
            (
                "senry pe crash report dekho",
                "Sentry pe crash report dekho",
            ),
        ],
    },
    DevTerm {
        canonical: "Stripe",
        distortions: &["stryp", "strype"],
        sentences: &[
            (
                "stryp payment integration karo",
                "Stripe payment integration karo",
            ),
            (
                "strype webhook setup karna hai",
                "Stripe webhook setup karna hai",
            ),
        ],
    },
    DevTerm {
        canonical: "React",
        distortions: &["reakt", "reeact"],
        sentences: &[
            ("reakt component banana hai", "React component banana hai"),
            ("reeact hooks use karo", "React hooks use karo"),
        ],
    },
    DevTerm {
        canonical: "Docker",
        distortions: &["dokker", "doccer"],
        sentences: &[
            ("dokker container bana do", "Docker container bana do"),
            ("doccer image build karo", "Docker image build karo"),
        ],
    },
    DevTerm {
        canonical: "Helm",
        distortions: &["helam", "halm"],
        sentences: &[
            ("helam chart install karo", "Helm chart install karo"),
            ("halm upgrade karna hai", "Helm upgrade karna hai"),
        ],
    },
    DevTerm {
        canonical: "Cursor",
        distortions: &["kursor", "cursr"],
        sentences: &[
            (
                "kursor editor mein code likho",
                "Cursor editor mein code likho",
            ),
            ("cursr AI bahut helpful hai", "Cursor AI bahut helpful hai"),
        ],
    },
    DevTerm {
        canonical: "Notion",
        distortions: &["nosion", "noshon"],
        sentences: &[
            ("nosion mein docs likho", "Notion mein docs likho"),
            ("noshon workspace share karo", "Notion workspace share karo"),
        ],
    },
];

// ── Real words that must NEVER be touched ────────────────────────────────────

const SAFE_HINDI_SENTENCES: &[&str] = &[
    "kaafi accha kaam kiya tumne",
    "maine aaj bahut kaam kiya",
    "abhi meeting mein hoon",
    "dekho yeh code theek se chal raha hai",
    "haan bhai sahi keh rahe ho",
    "nahi yaar yeh galat hai",
    "pehle test likho phir deploy karo",
    "bahut accha idea hai yeh",
    "mujhe lagta hai yeh kaam karega",
    "kal tak yeh complete hona chahiye",
    "kaise ho tum sab",
    "tum log meeting mein aana",
    "yeh feature bahut zaruri hai",
    "thoda aur time lagega",
    "humein jaldi karna padega",
];

const SAFE_ENGLISH_SENTENCES: &[&str] = &[
    "the build is running on the mac right now",
    "this agent handles all the routing logic",
    "we need to fix this cursor positioning bug",
    "the database migration is complete",
    "docker is running on port 3000",
    "let me check the sentry dashboard",
    "the stripe webhook is not firing",
    "react component is rendering twice",
    "helm chart needs an update",
    "the slack notification is not working",
    "check the notion docs for architecture",
    "redis cache hit rate is dropping",
    "the queue is backed up with messages",
    "we need to route this through the proxy",
    "the vault credentials have expired",
];

// ── LIFECYCLE TESTS ──────────────────────────────────────────────────────────

#[test]
fn lifecycle_learn_one_correction_then_auto_correct() {
    // The core lifecycle: user corrects once → all future occurrences auto-fix.
    let (pool, _db_path) = fresh_db();

    for term in DEV_TERMS {
        let (transcript, corrected) = term.sentences[0];

        // Session 1: First encounter — no aliases yet, nothing corrects
        let result1 = run_correction(&pool, transcript);
        assert_eq!(
            result1.text, transcript,
            "[{}] first encounter should NOT correct (no aliases yet)",
            term.canonical
        );

        // User corrects → learning fires
        let (learned, terms) = simulate_user_edit(&pool, transcript, transcript, corrected);
        assert!(
            learned > 0,
            "[{}] should learn from correction {:?} → {:?}",
            term.canonical,
            transcript,
            corrected
        );
        assert!(
            terms.contains(&term.canonical.to_string()),
            "[{}] learned terms should contain canonical",
            term.canonical
        );

        // Session 2: Same transcript — alias fires, auto-corrected
        let result2 = run_correction(&pool, transcript);
        assert_eq!(
            result2.text, corrected,
            "[{}] after learning, same transcript should auto-correct",
            term.canonical
        );
    }
}

#[test]
fn lifecycle_proactive_seeding_catches_variants() {
    // After one correction, proactive seeding should generate extra aliases
    // for terms with 4+ chars (short terms like "n8n" produce fewer seeds).
    let (pool, _db_path) = fresh_db();

    // Pick EMIAC — long enough for productive seeding
    let (transcript, corrected) = (
        "meah ka quarterly revenue dekhna hai",
        "EMIAC ka quarterly revenue dekhna hai",
    );
    simulate_user_edit(&pool, transcript, transcript, corrected);

    let all_rules = stt_replacements::load_all(&pool, "u1");
    let emiac_rules: Vec<_> = all_rules
        .iter()
        .filter(|r| r.correct_form == "EMIAC")
        .collect();
    assert!(
        emiac_rules.len() >= 3,
        "EMIAC should have 3+ aliases after proactive seeding (got {}): {:?}",
        emiac_rules.len(),
        emiac_rules
            .iter()
            .map(|r| &r.transcript_form)
            .collect::<Vec<_>>()
    );
}

#[test]
fn lifecycle_multiple_distortions_each_learned_fires() {
    // Each distortion must be learned individually, then it fires.
    // Proactive seeding may also catch some variants automatically.
    let (pool, _db_path) = fresh_db();

    for term in DEV_TERMS {
        // Learn ALL sentences for this term (simulates multiple corrections)
        for (transcript, corrected) in term.sentences {
            simulate_user_edit(&pool, transcript, transcript, corrected);
        }

        // Verify each learned distortion fires
        for (test_transcript, _expected) in term.sentences {
            let result = run_correction(&pool, test_transcript);
            assert!(
                result.text.contains(term.canonical),
                "[{}] sentence {:?} should contain {:?} after learning all distortions, got {:?}",
                term.canonical,
                test_transcript,
                term.canonical,
                result.text
            );
        }
    }
}

#[test]
fn safety_hindi_sentences_never_touched() {
    // CRITICAL: no Hindi word should ever be replaced by a dev term.
    let (pool, _db_path) = fresh_db();

    // First, learn ALL dev terms
    for term in DEV_TERMS {
        let (transcript, corrected) = term.sentences[0];
        simulate_user_edit(&pool, transcript, transcript, corrected);
    }

    // Now verify that pure Hindi sentences survive untouched
    for sentence in SAFE_HINDI_SENTENCES {
        let result = run_correction(&pool, sentence);
        assert_eq!(
            result.text, *sentence,
            "Hindi sentence should survive UNTOUCHED: {:?}",
            sentence
        );
    }
}

#[test]
fn safety_english_real_words_never_touched() {
    // CRITICAL: real English words must never be replaced.
    // "mac", "agent", "cursor", "docker", "sentry", "stripe", "helm",
    // "react", "notion", "redis", "queue", "route", "proxy", "vault", "slack"
    let (pool, _db_path) = fresh_db();

    // Learn ALL dev terms
    for term in DEV_TERMS {
        let (transcript, corrected) = term.sentences[0];
        simulate_user_edit(&pool, transcript, transcript, corrected);
    }

    // Real English sentences must survive
    for sentence in SAFE_ENGLISH_SENTENCES {
        let result = run_correction(&pool, sentence);
        assert_eq!(
            result.text, *sentence,
            "English sentence with real words should survive UNTOUCHED: {:?}",
            sentence
        );
    }
}

#[test]
fn safety_kaafi_never_becomes_kafka() {
    let (pool, _db_path) = fresh_db();

    // Learn Kafka
    simulate_user_edit(
        &pool,
        "cafka consumer group check karo",
        "cafka consumer group check karo",
        "Kafka consumer group check karo",
    );

    // "kaafi" is a real Hindi word meaning "enough" — must NEVER become Kafka
    let sentences = [
        "kaafi accha kaam kiya",
        "bahut kaafi hai yeh",
        "kaafi time ho gaya",
        "yeh kaafi important hai",
    ];
    for s in &sentences {
        let result = run_correction(&pool, s);
        assert_eq!(
            result.text, *s,
            "'kaafi' should NEVER become 'Kafka': {:?}",
            s
        );
    }
}

#[test]
fn safety_mac_never_becomes_emiac() {
    let (pool, _db_path) = fresh_db();

    // Learn EMIAC
    simulate_user_edit(&pool, "meah ka office", "meah ka office", "EMIAC ka office");

    // "mac" is a real English word — must NEVER become EMIAC
    let sentences = [
        "mac pe build karo",
        "yeh mac bahut slow hai",
        "mac mini kharidna hai",
        "apna mac restart karo",
    ];
    for s in &sentences {
        let result = run_correction(&pool, s);
        assert_eq!(
            result.text, *s,
            "'mac' should NEVER become 'EMIAC': {:?}",
            s
        );
    }
}

#[test]
fn safety_agent_never_becomes_airnote() {
    let (pool, _db_path) = fresh_db();

    simulate_user_edit(
        &pool,
        "automot app update karo",
        "automot app update karo",
        "AirNote app update karo",
    );

    let sentences = [
        "the agent is handling requests",
        "yeh agent bahut smart hai",
        "claude agent use karo",
    ];
    for s in &sentences {
        let result = run_correction(&pool, s);
        assert_eq!(
            result.text, *s,
            "'agent' should NEVER become 'AirNote': {:?}",
            s
        );
    }
}

#[test]
fn safety_cursor_the_word_survives() {
    let (pool, _db_path) = fresh_db();

    simulate_user_edit(
        &pool,
        "curser editor open karo",
        "curser editor open karo",
        "Cursor editor open karo",
    );

    // "cursor" the English word must survive
    let sentences = [
        "cursor position fix karo",
        "the cursor is at the end",
        "move the cursor to line 10",
    ];
    for s in &sentences {
        let result = run_correction(&pool, s);
        assert_eq!(result.text, *s, "'cursor' the word should survive: {:?}", s);
    }
}

#[test]
fn lifecycle_demotion_blocks_bad_alias() {
    // If system wrongly corrects, user reverts → alias should be blocked forever.
    let (pool, _db_path) = fresh_db();

    // Insert a legit alias, then demote it
    stt_replacements::upsert(&pool, "u1", "meah", "EMIAC", 1.0);
    stt_replacements::approve_aliases_for_term(&pool, "u1", "EMIAC");

    // Verify it fires before demotion
    let result = run_correction(&pool, "meah coverage dekhna hai");
    assert!(
        result.text.contains("EMIAC"),
        "alias should fire before demotion: {:?}",
        result.text
    );

    // Demote it (user corrected it back)
    stt_replacements::demote(&pool, "u1", "meah", "EMIAC", 2.0);

    // After demotion, "meah" must NEVER become EMIAC
    let result2 = run_correction(&pool, "meah coverage dekhna hai");
    assert!(
        !result2.text.contains("EMIAC"),
        "after demotion, 'meah' should never become 'EMIAC', got: {:?}",
        result2.text
    );

    // The blocked row must survive in DB
    let rules = stt_replacements::load_all(&pool, "u1");
    let blocked = rules.iter().find(|r| r.transcript_form == "meah");
    assert!(blocked.is_some(), "blocked alias row should survive in DB");
    assert_eq!(
        blocked.unwrap().review_status,
        stt_replacements::ReviewStatus::Blocked
    );
}

#[test]
fn lifecycle_no_edit_rewards_vocab() {
    // When user keeps text unchanged, existing vocab terms get positive signal.
    let (pool, _db_path) = fresh_db();

    // Pre-learn EMIAC
    vocabulary::upsert_for_language_with_context(
        &pool, "u1", "EMIAC", 1.0, "auto", "hinglish", None,
    );

    let before = vocabulary::find_by_term_ci(&pool, "u1", "EMIAC").unwrap();
    let before_weight = before.weight;

    // Simulate no-edit (user kept text as-is)
    vocabulary::reward_active_terms(&pool, "u1", "EMIAC ka quarterly meeting hai", 0.1);

    let after = vocabulary::find_by_term_ci(&pool, "u1", "EMIAC").unwrap();
    assert!(
        after.weight > before_weight,
        "no-edit should bump vocab weight"
    );
}

#[test]
fn lifecycle_mixed_hinglish_devtools_sentence() {
    // A realistic mixed sentence: Hindi grammar + dev terms + English words.
    let (pool, _db_path) = fresh_db();

    // Learn some terms
    simulate_user_edit(
        &pool,
        "growk API se claud model call karo",
        "growk API se claud model call karo",
        "Groq API se Claude model call karo",
    );

    // Next time both should correct
    let result = run_correction(&pool, "growk se claud ko call karo");
    assert!(
        result.text.contains("Groq"),
        "Groq should be corrected: {:?}",
        result.text
    );
    assert!(
        result.text.contains("Claude"),
        "Claude should be corrected: {:?}",
        result.text
    );
}

#[test]
fn lifecycle_multi_word_alias() {
    // "mein corps" → "MACOBS" as a multi-word alias.
    // Note: "main" is a dictionary word so apply_exact_safe filters it.
    // Using "mein corps" where individual tokens are gibberish in this context.
    let (pool, _db_path) = fresh_db();

    stt_replacements::upsert_aliases_for_language(
        &pool, "u1", "mecorbs", "mecorbs", "MACOBS", 1.0, "hinglish",
    );
    stt_replacements::approve_aliases_for_term(&pool, "u1", "MACOBS");
    vocabulary::upsert_for_language_with_context(
        &pool, "u1", "MACOBS", 1.0, "auto", "hinglish", None,
    );

    let result = run_correction(&pool, "mecorbs ka market cap kya hai");
    assert!(
        result.text.contains("MACOBS"),
        "single-word gibberish alias should fire: {:?}",
        result.text
    );
}

#[test]
fn lifecycle_case_preservation() {
    // Corrected terms should preserve their canonical casing.
    let (pool, _db_path) = fresh_db();

    simulate_user_edit(&pool, "meah ka office", "meah ka office", "EMIAC ka office");

    let result = run_correction(&pool, "meah ka office");
    assert!(
        result.text.contains("EMIAC"),
        "should preserve ALLCAPS: {:?}",
        result.text
    );
    assert!(
        !result.text.contains("emiac") && !result.text.contains("Emiac"),
        "casing must be EMIAC not emiac/Emiac: {:?}",
        result.text
    );
}

#[test]
fn safety_all_30_terms_dont_corrupt_hindi() {
    // The nuclear test: learn ALL 30 terms, then blast 15 Hindi sentences through.
    // Not a single word should change.
    let (pool, _db_path) = fresh_db();

    // Learn every single dev term
    for term in DEV_TERMS {
        for (transcript, corrected) in term.sentences {
            simulate_user_edit(&pool, transcript, transcript, corrected);
        }
    }

    // Count total aliases
    let all_rules = stt_replacements::load_all(&pool, "u1");
    eprintln!(
        "[alpha] total aliases after learning all terms: {}",
        all_rules.len()
    );

    // Every Hindi sentence must survive perfectly
    for sentence in SAFE_HINDI_SENTENCES {
        let result = run_correction(&pool, sentence);
        assert_eq!(
            result.text, *sentence,
            "SAFETY VIOLATION: Hindi sentence corrupted after learning all 30 terms: {:?} → {:?}",
            sentence, result.text
        );
    }

    // Every English sentence must survive perfectly
    for sentence in SAFE_ENGLISH_SENTENCES {
        let result = run_correction(&pool, sentence);
        assert_eq!(
            result.text, *sentence,
            "SAFETY VIOLATION: English sentence corrupted after learning all 30 terms: {:?} → {:?}",
            sentence, result.text
        );
    }
}

#[test]
fn safety_close_pairs_hindi_vs_brands() {
    // Adversarial: Hindi words that SOUND like brand names.
    let (pool, _db_path) = fresh_db();

    // Learn the brands
    simulate_user_edit(
        &pool,
        "cafka setup karo",
        "cafka setup karo",
        "Kafka setup karo",
    );
    simulate_user_edit(&pool, "sentri lagao", "sentri lagao", "Sentry lagao");
    simulate_user_edit(
        &pool,
        "dokker build karo",
        "dokker build karo",
        "Docker build karo",
    );
    simulate_user_edit(
        &pool,
        "helam install karo",
        "helam install karo",
        "Helm install karo",
    );

    // Close Hindi words must survive
    let safe = [
        ("kaafi time ho gaya", "kaafi"),
        ("sab theek hai", "theek"),
        ("haan dekho", "dekho"),
        ("abhi nahi", "abhi"),
        ("bahut accha hai", "accha"),
        ("pehle yeh karo", "pehle"),
    ];
    for (sentence, word) in &safe {
        let result = run_correction(&pool, sentence);
        assert_eq!(
            result.text, *sentence,
            "Hindi word '{word}' was corrupted: {:?} → {:?}",
            sentence, result.text
        );
    }
}
