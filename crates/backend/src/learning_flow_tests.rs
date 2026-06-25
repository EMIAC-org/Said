//! End-to-end tests for the learning pipeline.
//!
//! These tests bypass the network call to Groq.  They build a `LabelledHunk`
//! the same way the new pipeline would after diff + LLM, then drive it
//! through the same promotion/gate code the live route uses.  This covers:
//!   1. STT_ERROR + jargon candidate → vocabulary + stt_replacement written
//!   2. Demotion of starred terms is rejected
//!   3. The email-link bug never reaches promotion (caught at pre_filter)
//!   4. The Devanagari-hallucination bug never reaches promotion (caught at
//!      diff stage — hallucinated terms simply aren't in the diff hunks)
//!   5. Repeat promotion bumps use_count, doesn't duplicate

#![cfg(test)]

use crate::llm::{
    classifier::{EditClass, ExtractedTerm, LabelledHunk},
    edit_diff::{self, Hunk},
    phonetics, pre_filter, promotion_gate,
};
use crate::store::{DbPool, stt_replacements, vocabulary};
use r2d2_sqlite::SqliteConnectionManager;

fn pool() -> DbPool {
    crate::legacy_learning::enable_debug_legacy_writes_for_tests();
    let mgr = SqliteConnectionManager::memory();
    let pool = r2d2::Pool::builder().max_size(2).build(mgr).unwrap();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "CREATE TABLE local_user (id TEXT PRIMARY KEY);
         INSERT INTO local_user(id) VALUES ('u1');

         CREATE TABLE vocabulary (
             user_id                 TEXT NOT NULL REFERENCES local_user(id),
             term                    TEXT NOT NULL,
             weight                  REAL NOT NULL DEFAULT 1.0,
             use_count               INTEGER NOT NULL DEFAULT 1,
             last_used               INTEGER NOT NULL,
             source                  TEXT NOT NULL DEFAULT 'auto',
             language                TEXT,
             example_context         TEXT,
             term_type               TEXT,
             meaning                 TEXT,
             meaning_updated_at      INTEGER,
             examples_since_meaning  INTEGER NOT NULL DEFAULT 0,
             UNIQUE(user_id, term)
         );

         CREATE TABLE stt_replacements (
             user_id          TEXT NOT NULL,
             transcript_form  TEXT NOT NULL,
             correct_form     TEXT NOT NULL,
             phonetic_key     TEXT NOT NULL,
             weight           REAL NOT NULL DEFAULT 1.0,
             use_count        INTEGER NOT NULL DEFAULT 1,
             last_used        INTEGER NOT NULL,
             language         TEXT,
             export_tier      TEXT NOT NULL DEFAULT 'local_only',
             contradiction_count INTEGER NOT NULL DEFAULT 0,
             last_contradicted_at INTEGER,
             review_status    TEXT NOT NULL DEFAULT 'pending',
             review_reason    TEXT,
             last_reviewed_at INTEGER,
             UNIQUE(user_id, transcript_form, correct_form)
         );

         CREATE TABLE word_corrections (
             user_id      TEXT NOT NULL,
             wrong_text   TEXT NOT NULL,
             correct_text TEXT NOT NULL,
             count        INTEGER NOT NULL DEFAULT 1,
             updated_at   INTEGER NOT NULL,
             weight       REAL NOT NULL DEFAULT 1.0,
             tier         TEXT NOT NULL DEFAULT 'enforced',
             UNIQUE(user_id, wrong_text)
         );",
    )
    .unwrap();
    pool
}

/// Build a labelled hunk identical to what the full pipeline would produce.
fn label(hunk: Hunk, class: EditClass, confidence: f64) -> LabelledHunk {
    LabelledHunk {
        hunk,
        class,
        confidence,
        extracted_term: None,
    }
}

/// Build a labelled hunk with an extracted_term (the LLM identified a specific
/// sub-token within the hunk as the actual correction).
fn label_with_extract(
    hunk: Hunk,
    class: EditClass,
    confidence: f64,
    transcript_form: &str,
    correct_form: &str,
) -> LabelledHunk {
    LabelledHunk {
        hunk,
        class,
        confidence,
        extracted_term: Some(ExtractedTerm {
            transcript_form: transcript_form.into(),
            correct_form: correct_form.into(),
        }),
    }
}

/// Stage-4 promotion logic mirroring `routes::classify::classify` (no network).
/// Includes the full gate stack: capture-confidence, script, phonetic/jargon,
/// concatenation, appears-in-user-kept.
fn promote(
    pool: &DbPool,
    user_id: &str,
    user_kept: &str,
    output_language: &str,
    cands: &[LabelledHunk],
) -> (usize, bool) {
    promote_with_capture(pool, user_id, user_kept, output_language, cands, "ax")
}

fn promote_with_capture(
    pool: &DbPool,
    user_id: &str,
    user_kept: &str,
    output_language: &str,
    cands: &[LabelledHunk],
    capture_method: &str,
) -> (usize, bool) {
    // Capture-confidence master gate — mirrors the route.
    let auto_promote_allowed = matches!(capture_method, "ax" | "keystroke_verified" | "clipboard");
    if !auto_promote_allowed {
        return (0, false);
    }

    let mut promoted = 0_usize;
    let mut learned = false;
    for cand in cands {
        if cand.class != EditClass::SttError {
            continue;
        }
        let correct = cand.correct_form().trim();
        if correct.is_empty() {
            continue;
        }

        if !promotion_gate::appears_in_user_kept(correct, user_kept) {
            continue;
        }
        if !promotion_gate::script_matches(correct, output_language) {
            continue;
        }

        // Concatenation guard — same as the route.
        if cand.extracted_term.is_none()
            && promotion_gate::is_concatenation_pattern(cand.polish_form(), correct)
        {
            continue;
        }

        let phon_sim = phonetics::similarity(cand.transcript_form(), correct)
            .max(phonetics::similarity(cand.polish_form(), correct));
        let jargon = phonetics::jargon_score(correct);
        let confident = cand.confidence >= 0.7;
        if phon_sim < 0.5 && jargon < 0.4 && !confident {
            continue;
        }

        if vocabulary::upsert(pool, user_id, correct, 1.0, "auto") {
            learned = true;
            promoted += 1;
        }
        let from = cand.transcript_form().trim();
        if !from.is_empty()
            && !from.eq_ignore_ascii_case(correct)
            && stt_replacements::upsert(pool, user_id, from, correct, 1.0)
        {
            promoted += 1;
        }
    }
    (promoted, learned)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn n8n_case_promotes_via_full_pipeline() {
    let pool = pool();
    let polish = "I use written for automation";
    let user_kept = "I use n8n for automation";
    let transcript = polish; // STT misheard

    // Stage 1 — pre-filter must let this through.
    assert_eq!(
        pre_filter::run(polish, user_kept, "hinglish"),
        pre_filter::PreFilter::Pass
    );

    // Stage 2 — diff produces exactly one hunk with concrete text.
    let hunks = edit_diff::diff(transcript, polish, user_kept);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].polish_window, "written");
    assert_eq!(hunks[0].kept_window, "n8n");

    // Stage 3 — labeler (simulated) tags as STT_ERROR.
    let cands = vec![label(hunks[0].clone(), EditClass::SttError, 0.9)];

    // Stage 4 — promotion.
    let (promoted, learned) = promote(&pool, "u1", user_kept, "hinglish", &cands);
    assert_eq!(promoted, 2, "vocab + stt_replacement");
    assert!(learned);
    assert!(vocabulary::top_term_strings(&pool, "u1", 10).contains(&"n8n".to_string()));
}

#[test]
fn email_link_prefix_bug_blocked_at_pre_filter() {
    // The exact production failure that prompted this rebuild.
    let polish = "Anish at Gmail dot com ka zara batana kaun sa mail ID par bhejna hai";
    let kept = "[anish@gmail.com](mailto:anish@gmail.com) Anish at Gmail dot com ka zara batana kaun sa mail ID par bhejna hai";

    match pre_filter::run(polish, kept, "hinglish") {
        pre_filter::PreFilter::EarlyClass(d) => assert_eq!(d.class, "USER_REWRITE"),
        other => panic!("expected USER_REWRITE early-exit, got {other:?}"),
    }
    // Pipeline never even reaches the LLM — no hallucinated promotions possible.
}

#[test]
fn devanagari_hallucination_blocked_at_diff_stage() {
    // Even if pre_filter let it through (suppose it did), the diff stage
    // produces hunks whose text is taken from the actual strings — the
    // hallucinated "अनीष / का / ज़रा" candidates from the Groq bug cannot
    // appear because they don't exist in any of the three texts.
    let transcript = "Anish at Gmail dot com ka zara batana";
    let polish = "Anish at Gmail dot com ka zara batana";
    let kept = "[anish@gmail.com](mailto:anish@gmail.com) Anish at Gmail dot com ka zara batana";

    let hunks = edit_diff::diff(transcript, polish, kept);
    for h in &hunks {
        assert!(!h.kept_window.contains("अनीष"));
        assert!(!h.kept_window.contains("का"));
        assert!(!h.kept_window.contains("ज़रा"));
    }
}

#[test]
fn devanagari_in_hinglish_mode_blocked_by_script_gate() {
    // If a hunk did somehow surface a Devanagari "correct_form" (e.g. user
    // genuinely typed Devanagari mid-sentence), the script gate refuses to
    // promote in Hinglish mode.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "ka".into(),
        polish_window: "ka".into(),
        kept_window: "का".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.9)];
    // user_kept contains the Devanagari char so appears_in_user_kept passes,
    // but script_matches must reject it under hinglish mode.
    let (promoted, learned) = promote(&pool, "u1", "ka का batana", "hinglish", &cands);
    assert_eq!(promoted, 0);
    assert!(!learned);
    assert_eq!(vocabulary::count(&pool, "u1"), 0);
}

#[test]
fn hallucinated_correct_form_blocked_by_appears_in_user_kept() {
    // Defense in depth: even if labelling is wrong, a candidate whose
    // correct_form doesn't actually appear in user_kept never promotes.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    // But suppose by some bug the labelled hunk's kept_window claimed a value
    // that never appeared in the actual user_kept string we hand the gate.
    let cands = vec![label(hunk, EditClass::SttError, 0.95)];
    let (promoted, _) = promote(&pool, "u1", "I use something different", "hinglish", &cands);
    assert_eq!(
        promoted, 0,
        "n8n is not in user_kept — promotion must be refused"
    );
}

#[test]
fn repeat_promotion_bumps_use_count_not_duplicate_row() {
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.9)];
    let kept = "I use n8n for automation";
    promote(&pool, "u1", kept, "hinglish", &cands);
    promote(&pool, "u1", kept, "hinglish", &cands);
    let terms = vocabulary::top_terms(&pool, "u1", 10);
    let n8n = terms.iter().find(|t| t.term == "n8n").expect("n8n missing");
    assert_eq!(n8n.use_count, 2);
    assert!(n8n.weight > 1.0);
}

#[test]
fn weak_signal_stt_candidate_is_dropped() {
    // Common short word, low confidence, no jargon score, no phonetic match —
    // the gate must refuse.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "good".into(),
        polish_window: "good".into(),
        kept_window: "ok".into(), // common, jargon=0
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.3)];
    let (promoted, _) = promote(&pool, "u1", "ok then", "english", &cands);
    assert_eq!(promoted, 0);
}

#[test]
fn anish_email_link_extracts_just_the_name() {
    // The exact production case from the user's logs:
    //   polish: "Anis at the rate Gmail dot com ko dekhna. Zara mail bhej..."
    //   kept:   "[anish@gmail.com](mailto:anish@gmail.com) ko dekhna. Zara mail bhej sakte ho?"
    //
    // The hunk's kept_window contains the WHOLE markdown link (which is
    // REWRITE shape), but the LLM correctly identifies that only "anish" is
    // the learnable STT correction.  With extracted_term, promotion picks up
    // ONLY "anish" — the link wrapping is ignored.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "Anis at the rate Gmail dot com".into(),
        polish_window: "Anis at the rate Gmail dot com".into(),
        kept_window: "[anish@gmail.com](mailto:anish@gmail.com)".into(),
    };
    let cands = vec![label_with_extract(
        hunk,
        EditClass::SttError,
        0.9,
        "Anis",
        "anish",
    )];
    let kept = "[anish@gmail.com](mailto:anish@gmail.com) ko dekhna. Zara mail bhej sakte ho?";

    let (promoted, learned) = promote(&pool, "u1", kept, "hinglish", &cands);
    assert!(learned, "anish should be learned via extracted_term");
    assert!(promoted >= 1);

    // The vocabulary now contains "anish" — NOT the whole markdown link.
    let terms = vocabulary::top_term_strings(&pool, "u1", 10);
    assert!(
        terms.iter().any(|t| t == "anish"),
        "vocab should contain 'anish' alone, got: {terms:?}"
    );
    assert!(
        !terms.iter().any(|t| t.contains("[")),
        "vocab must NOT contain the markdown link as a term"
    );
    assert!(
        !terms.iter().any(|t| t.contains("mailto")),
        "vocab must NOT contain 'mailto' as a term"
    );
}

#[test]
fn extracted_term_overrides_kept_window_in_correct_form() {
    // Even at the unit level: the getter must prefer extracted_term.correct_form.
    let cand = label_with_extract(
        Hunk {
            transcript_window: "Anis ...".into(),
            polish_window: "Anis ...".into(),
            kept_window: "[anish@gmail.com](mailto:anish@gmail.com)".into(),
        },
        EditClass::SttError,
        0.9,
        "Anis",
        "anish",
    );
    assert_eq!(cand.correct_form(), "anish");
    assert_eq!(cand.transcript_form(), "Anis");
}

#[test]
fn emiac_concatenation_blocked_by_concatenation_gate() {
    // Production case: user wanted to replace "MAAR" with "EMIAC" but the
    // captured edit shows "EMIACMAAR" because keystroke replay missed the
    // selection event in an AX-blind app.  The concatenation gate alone
    // (independent of capture confidence) must refuse this promotion.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "MAAR".into(),
        polish_window: "MAAR".into(),
        kept_window: "EMIACMAAR".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.85)];
    let kept = "EMIACMAAR technologies ka kal IPO nikalne wala hai";

    // Even with full "ax" confidence, the concatenation gate refuses promotion.
    let (promoted, learned) = promote(&pool, "u1", kept, "english", &cands);
    assert_eq!(promoted, 0, "concatenation pattern must not promote");
    assert!(!learned);
    assert_eq!(vocabulary::count(&pool, "u1"), 0);
}

#[test]
fn keystroke_only_capture_blocks_all_promotion() {
    // Foundational guard: when the desktop reports `keystroke_only`
    // (clipboard verification was unavailable, so the captured edit might be
    // a reconstruction artifact), NO promotion fires regardless of class.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.95)];
    let (promoted, learned) = promote_with_capture(
        &pool,
        "u1",
        "I use n8n daily",
        "english",
        &cands,
        "keystroke_only",
    );
    assert_eq!(promoted, 0, "low-confidence capture must not auto-promote");
    assert!(!learned);
    assert_eq!(
        vocabulary::count(&pool, "u1"),
        0,
        "vocab must remain empty under keystroke_only confidence"
    );
}

#[test]
fn keystroke_verified_capture_does_promote() {
    // When the desktop cross-verified keystroke replay against clipboard and
    // they agreed, capture confidence is high — promotion proceeds normally.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.95)];
    let (promoted, learned) = promote_with_capture(
        &pool,
        "u1",
        "I use n8n",
        "english",
        &cands,
        "keystroke_verified",
    );
    assert!(promoted >= 1);
    assert!(learned);
    assert!(vocabulary::top_term_strings(&pool, "u1", 10).contains(&"n8n".to_string()));
}

#[test]
fn clipboard_capture_does_promote() {
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.9)];
    let (promoted, learned) =
        promote_with_capture(&pool, "u1", "I use n8n", "english", &cands, "clipboard");
    assert!(promoted >= 1);
    assert!(learned);
}

#[test]
fn unknown_capture_method_defaults_to_blocked() {
    // Defensive: any unknown capture_method string is treated as low-confidence.
    let pool = pool();
    let hunk = Hunk {
        transcript_window: "written".into(),
        polish_window: "written".into(),
        kept_window: "n8n".into(),
    };
    let cands = vec![label(hunk, EditClass::SttError, 0.9)];
    let (promoted, _) =
        promote_with_capture(&pool, "u1", "I use n8n", "english", &cands, "future_method");
    assert_eq!(promoted, 0);
}

#[test]
fn starred_vocabulary_immune_to_demotion() {
    let pool = pool();
    vocabulary::upsert(&pool, "u1", "ProjectAtlas", 1.0, "starred");
    for _ in 0..5 {
        vocabulary::demote(&pool, "u1", "ProjectAtlas", 1.0);
    }
    assert_eq!(vocabulary::count(&pool, "u1"), 1);
}
