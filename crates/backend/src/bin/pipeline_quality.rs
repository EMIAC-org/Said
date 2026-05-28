//! Full-pipeline quality test suite for AirNote.
//!
//! Runs real Deepgram transcripts through the REAL selection gates + Groq LLM.
//! Vocab entries go through the same phonetic/BM25/anchor gates as production.
//! Only entries that PASS selection reach the LLM prompt.
//!
//! Usage:
//!   GROQ_API_KEY=gsk_... cargo run -p said-backend --bin pipeline-quality
//!
//! Rate-limit aware: 2-second delay between Groq calls (free tier = 30 RPM).

use said_backend::llm::phonetics;
use said_backend::llm::prompt::{
    VocabEntry, VocabResolution, build_user_message, default_voice_prompt_template,
    render_voice_system_prompt_template,
};
use said_backend::store::prefs::Preferences;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Instant;

// ═══════════════════════════════════════════════════════════════════════════
//  SIMULATED VOCAB TERM (mirrors VocabTerm from DB)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct SimVocabTerm {
    term: String,
    term_type: Option<String>,
    meaning: Option<String>,
    example_context: Option<String>,
    stt_aliases: Vec<(String, i64)>,
}

impl SimVocabTerm {
    fn to_prompt_entry(&self, resolution: VocabResolution) -> VocabEntry {
        VocabEntry {
            term: self.term.clone(),
            term_type: self.term_type.clone(),
            meaning: self.meaning.clone(),
            context: self.example_context.clone(),
            resolution,
            stt_aliases: self.stt_aliases.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  REAL SELECTION GATE (mirrors vocab_embeddings.rs lines 746-868)
// ═══════════════════════════════════════════════════════════════════════════

struct SelectionResult {
    term: String,
    selected: bool,
    gate_passed: &'static str,
}

fn simulate_selection(vocab: &[SimVocabTerm], transcript: &str) -> Vec<SelectionResult> {
    let transcript_lower = transcript.to_lowercase();
    let transcript_tokens: Vec<&str> = transcript_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    vocab
        .iter()
        .map(|vt| {
            let term_lower = vt.term.to_ascii_lowercase();

            // Gate 1: meaning must exist
            let has_meaning = vt
                .meaning
                .as_deref()
                .map(|m| !m.trim().is_empty())
                .unwrap_or(false);
            if !has_meaning {
                return SelectionResult {
                    term: vt.term.clone(),
                    selected: false,
                    gate_passed: "BLOCKED: no meaning",
                };
            }

            // NEW ARCHITECTURE: Bucket A always includes top-weight terms.
            // In the test, all available_vocab terms represent the user's
            // learned vocabulary. With the new pipeline, they all get
            // included and the LLM decides contextually.
            // We still log which matching path WOULD have caught it for
            // diagnostic purposes.

            let term_words: Vec<&str> = term_lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect();

            // Path A: whole-word match
            let whole_word = if term_words.len() == 1 {
                transcript_tokens.iter().any(|tok| *tok == term_words[0])
            } else {
                transcript_tokens
                    .windows(term_words.len())
                    .any(|w| w == term_words.as_slice())
            };
            if whole_word {
                return SelectionResult {
                    term: vt.term.clone(),
                    selected: true,
                    gate_passed: "whole-word",
                };
            }

            // Path A2i: STT alias exact-match
            let alias_exact = vt.stt_aliases.iter().any(|(alias, _)| {
                let a = alias.to_ascii_lowercase();
                transcript_tokens.iter().any(|tok| *tok == a.as_str())
            });
            if alias_exact {
                return SelectionResult {
                    term: vt.term.clone(),
                    selected: true,
                    gate_passed: "alias-exact",
                };
            }

            // Path A2ii: alias SUBSTRING match (Deepgram joins words)
            let alias_substring = vt.stt_aliases.iter().any(|(alias, _)| {
                let a = alias.to_ascii_lowercase();
                if a.len() < 3 {
                    return false;
                }
                transcript_tokens
                    .iter()
                    .any(|tok| tok.len() > a.len() && tok.contains(a.as_str()))
            });
            if alias_substring {
                return SelectionResult {
                    term: vt.term.clone(),
                    selected: true,
                    gate_passed: "alias-substring",
                };
            }

            // Path A2iii: fuzzy alias match (novel distortions close to known alias)
            let alias_fuzzy = vt.stt_aliases.iter().any(|(alias, _)| {
                let a = alias.to_ascii_lowercase();
                if a.len() < 3 {
                    return false;
                }
                let a_phon = phonetics::phonetic_key(&a);
                transcript_tokens.iter().any(|tok| {
                    if tok.len() < 3 {
                        return false;
                    }
                    let tok_phon = phonetics::phonetic_key(tok);
                    phonetics::similarity(&tok_phon, &a_phon) >= 0.65
                })
            });
            if alias_fuzzy {
                return SelectionResult {
                    term: vt.term.clone(),
                    selected: true,
                    gate_passed: "alias-fuzzy",
                };
            }

            // Path B: phonetic match against TERM (with Devanagari stripping)
            if term_lower.len() >= 4 {
                let term_phon = phonetics::phonetic_key(&vt.term);
                for tok in &transcript_tokens {
                    if tok.len() < 3 {
                        continue;
                    }
                    let min_sim = if tok.len() <= 4 { 0.65 } else { 0.75 };
                    let tok_phon = phonetics::phonetic_key(tok);
                    if phonetics::similarity(&tok_phon, &term_phon) >= min_sim {
                        return SelectionResult {
                            term: vt.term.clone(),
                            selected: true,
                            gate_passed: "phonetic",
                        };
                    }
                    // Try with Devanagari stripped
                    let ascii_only: String =
                        tok.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                    if ascii_only.len() >= 3 && ascii_only.as_str() != *tok {
                        let ascii_phon = phonetics::phonetic_key(&ascii_only);
                        let ascii_sim = if ascii_only.len() <= 4 { 0.65 } else { 0.75 };
                        if phonetics::similarity(&ascii_phon, &term_phon) >= ascii_sim {
                            return SelectionResult {
                                term: vt.term.clone(),
                                selected: true,
                                gate_passed: "phonetic-devanagari-stripped",
                            };
                        }
                    }
                }
            }

            // Path C: context anchor matching
            if let Some(ctx) = vt.example_context.as_deref() {
                if !ctx.trim().is_empty()
                    && !matches!(vt.term_type.as_deref(), Some("other") | None)
                {
                    let term_tokens_set: HashSet<&str> = term_words.iter().copied().collect();
                    let anchors: HashSet<String> = ctx
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|w| !w.is_empty())
                        .map(|w| w.to_ascii_lowercase())
                        .filter(|w| *w != term_lower)
                        .filter(|w| !term_tokens_set.contains(w.as_str()))
                        .filter(|w| w.chars().any(|c| c.is_ascii_digit()) || w.len() >= 3)
                        .collect();
                    if anchors.len() >= 3 {
                        let matched = anchors
                            .iter()
                            .filter(|a| transcript_tokens.iter().any(|tok| *tok == a.as_str()))
                            .count();
                        if matched >= 3 {
                            return SelectionResult {
                                term: vt.term.clone(),
                                selected: true,
                                gate_passed: "context-anchor",
                            };
                        }
                    }
                }
            }

            // Path D: email fragment matching
            if term_lower.contains('@') {
                let has_email_signal = transcript_tokens.iter().any(|tok| {
                    matches!(
                        *tok,
                        "rate"
                            | "gmail"
                            | "yahoo"
                            | "outlook"
                            | "hotmail"
                            | "email"
                            | "mail"
                            | "protonmail"
                    )
                });
                if has_email_signal {
                    let matched_fragments = term_words
                        .iter()
                        .filter(|frag| frag.len() >= 3)
                        .filter(|frag| {
                            transcript_tokens.iter().any(|tok| {
                                *tok == **frag
                                    || (tok.len() >= 3
                                        && frag.len() >= 3
                                        && (tok.starts_with(*frag) || frag.starts_with(tok)))
                            })
                        })
                        .count();
                    if matched_fragments >= 2 {
                        return SelectionResult {
                            term: vt.term.clone(),
                            selected: true,
                            gate_passed: "email-fragment",
                        };
                    }
                }
            }

            // Compute phonetic details for debug output
            let term_phon = phonetics::phonetic_key(&vt.term);
            let best_tok = transcript_tokens
                .iter()
                .filter(|t| t.len() >= 3)
                .map(|t| {
                    let tp = phonetics::phonetic_key(t);
                    let sim = phonetics::similarity(&tp, &term_phon);
                    (*t, tp, sim)
                })
                .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                .map(|(tok, tp, sim)| format!("best='{tok}' key={tp} sim={sim:.2}"))
                .unwrap_or_default();

            // Bucket A: always include (top-weight). No gate blocked it —
            // the LLM decides contextually whether to apply the term.
            SelectionResult {
                term: vt.term.clone(),
                selected: true,
                gate_passed: Box::leak(
                    format!("top-weight (no phonetic match: {best_tok})").into_boxed_str(),
                ),
            }
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
//  TEST CASE DEFINITION
// ═══════════════════════════════════════════════════════════════════════════

struct TestCase {
    id: &'static str,
    category: &'static str,
    description: &'static str,
    transcript: &'static str,
    must_contain: &'static [&'static str],
    must_not_contain: &'static [&'static str],
    available_vocab: Vec<SimVocabTerm>,
    screen_context: Option<&'static str>,
}

fn emiac_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Emiac".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Emiac — an Indian technology company".to_string()),
        example_context: Some("Emiac technology mein kaam karte hain".to_string()),
        stt_aliases: vec![
            ("meac".to_string(), 5),
            ("meah".to_string(), 3),
            ("miac".to_string(), 2),
        ],
    }
}

fn macobs_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Macobs".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Macobs Technologies — an Indian SME company".to_string()),
        example_context: Some("Macobs ka IPO aane wala hai".to_string()),
        stt_aliases: vec![("mccorb".to_string(), 2)],
    }
}

fn email_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "vabhi.verma2678@gmail.com".to_string(),
        term_type: Some("proper_noun".to_string()),
        meaning: Some("Abhishek Verma's personal email address".to_string()),
        example_context: Some("vabhi.verma2678@gmail.com pe mail karo".to_string()),
        stt_aliases: vec![],
    }
}

// ── Diverse proper nouns (realistic user vocabulary) ─────────────────────

fn perplexity_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Perplexity".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Perplexity AI — an AI search engine and answer tool".to_string()),
        example_context: Some("Perplexity AI se automation banana hai".to_string()),
        stt_aliases: vec![
            ("perplex city".to_string(), 2),
            ("perplexed".to_string(), 1),
        ],
    }
}

fn vipassana_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Vipassana".to_string(),
        term_type: Some("proper_noun".to_string()),
        meaning: Some("Vipassana — a 10-day silent meditation retreat".to_string()),
        example_context: Some("Vipassana retreat mein jana hai next month".to_string()),
        stt_aliases: vec![("we passed na".to_string(), 3), ("vipassna".to_string(), 1)],
    }
}

fn razorpay_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Razorpay".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Razorpay — Indian payment gateway for online transactions".to_string()),
        example_context: Some("Razorpay ka integration karna hai payment ke liye".to_string()),
        stt_aliases: vec![("razor pay".to_string(), 2), ("razer pay".to_string(), 1)],
    }
}

fn kubernetes_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Kubernetes".to_string(),
        term_type: Some("code_identifier".to_string()),
        meaning: Some("Kubernetes — container orchestration platform, also called k8s".to_string()),
        example_context: Some("Kubernetes cluster pe deploy karna hai".to_string()),
        stt_aliases: vec![
            ("cougar netties".to_string(), 1),
            ("cooper naughties".to_string(), 1),
        ],
    }
}

fn zerodha_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Zerodha".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Zerodha — India's largest discount stock broker".to_string()),
        example_context: Some("Zerodha pe stocks buy karna hai".to_string()),
        stt_aliases: vec![("zero da".to_string(), 2), ("zeroda".to_string(), 1)],
    }
}

fn supabase_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Supabase".to_string(),
        term_type: Some("brand".to_string()),
        meaning: Some("Supabase — open-source Firebase alternative, Postgres backend".to_string()),
        example_context: Some("Supabase se backend banana hai instead of Firebase".to_string()),
        stt_aliases: vec![("super base".to_string(), 2), ("supa base".to_string(), 1)],
    }
}

fn anish_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "Anish".to_string(),
        term_type: Some("proper_noun".to_string()),
        meaning: Some("Anish Suman — colleague, developer at Emiac".to_string()),
        example_context: Some("Anish ko bolo ki code review kar de".to_string()),
        stt_aliases: vec![("anis".to_string(), 3), ("anees".to_string(), 1)],
    }
}

fn n8n_term() -> SimVocabTerm {
    SimVocabTerm {
        term: "n8n".to_string(),
        term_type: Some("code_identifier".to_string()),
        meaning: Some("n8n — workflow automation tool, open source".to_string()),
        example_context: Some("n8n se automation flow banana hai".to_string()),
        stt_aliases: vec![("aneten".to_string(), 2), ("written".to_string(), 1)],
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ALL TEST CASES
// ═══════════════════════════════════════════════════════════════════════════

fn build_test_cases() -> Vec<TestCase> {
    vec![
        // ─── PROPER NOUN DISTORTIONS (Emiac) ─────────────────────────────
        TestCase {
            id: "PN1",
            category: "proper_noun",
            description: "meac should become Emiac",
            transcript: "meac और में कितना काम हो गया",
            must_contain: &["Emiac"],
            must_not_contain: &["meac", "MEAC"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        TestCase {
            id: "PN2",
            category: "proper_noun",
            description: "meac technologies → Emiac Technologies",
            transcript: "मैं meac technologies में काम करता हूं",
            must_contain: &["Emiac"],
            must_not_contain: &["meac", "MEAC"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        TestCase {
            id: "PN3",
            category: "proper_noun",
            description: "meac must NOT become MNC",
            transcript: "यह जो meac जैसी बड़ी companies होती हैं ना इनका कुछ नहीं होने वाला भाई",
            must_contain: &["Emiac"],
            must_not_contain: &["MNC", "meac", "MEAC"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        TestCase {
            id: "PN4",
            category: "proper_noun",
            description: "m.e.a.c and मैकॉफ्स both distorted",
            transcript: "m.e.a.c और मैकॉफ्स में कितना काम हो गया",
            must_contain: &["Emiac"],
            must_not_contain: &["m.e.a.c", "macOS"],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        TestCase {
            id: "PN5",
            category: "proper_noun",
            description: "Both Macobs and Emiac correctly",
            transcript: "macobs जो है emiac के अंदर आती है",
            must_contain: &["Macobs", "Emiac"],
            must_not_contain: &[],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        TestCase {
            id: "PN6",
            category: "proper_noun",
            description: "Both in complex sentence",
            transcript: "मैं एक macobs technologies नाम की company के अंदर काम करता हूं जो कि emiac technologies के अंदर आती है",
            must_contain: &["Macobs", "Emiac"],
            must_not_contain: &[],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        // ─── HOMOPHONE PRESERVATION ──────────────────────────────────────
        TestCase {
            id: "HP1",
            category: "homophone",
            description: "main = I pronoun, NOT Emiac",
            transcript: "main kal office jaunga",
            must_contain: &["kal", "office"],
            must_not_contain: &["Emiac"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        TestCase {
            id: "HP2",
            category: "homophone",
            description: "maine = I did, NOT Emiac",
            transcript: "मैंने खाना खा लिया है",
            must_contain: &["khana"],
            must_not_contain: &["Emiac"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        TestCase {
            id: "HP3",
            category: "homophone",
            description: "mac = MacBook, NOT Macobs",
            transcript: "emiac में mac पर ही mostly काम होता है",
            must_contain: &["Emiac"],
            must_not_contain: &["Macobs"],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        // ─── EMAIL FOLDING ───────────────────────────────────────────────
        TestCase {
            id: "EF1",
            category: "email",
            description: "Spoken email should be folded",
            transcript: "मेरा email है v a b dot वर्मा two six seven eight at the rate gmail dot com",
            must_contain: &["@gmail.com"],
            must_not_contain: &["at the rate"],
            available_vocab: vec![email_term()],
            screen_context: None,
        },
        TestCase {
            id: "EF2",
            category: "email",
            description: "Email + proper noun combined",
            transcript: "अभिषेक वर्मा का email है b a b dot वर्मा two six seven eight at the rate gmail dot com और meac का रेवेन्यू पांच करोड़ cross कर गया है",
            must_contain: &["@gmail.com", "Emiac"],
            must_not_contain: &["at the rate", "meac"],
            available_vocab: vec![emiac_term(), email_term()],
            screen_context: None,
        },
        // ─── NUMBERS ────────────────────────────────────────────────────
        TestCase {
            id: "NC1",
            category: "numbers",
            description: "Invoice with mixed numbers",
            transcript: "invoice number two three zero five में तीन हज़ार six hundred रुपए का amount है",
            must_contain: &["2305"],
            must_not_contain: &["two three zero five"],
            available_vocab: vec![],
            screen_context: None,
        },
        TestCase {
            id: "NC2",
            category: "numbers",
            description: "twenty sixth May → 26th May",
            transcript: "macobs का ipo और emiac partnership दोनों एक साथ announce होगा twenty sixth may",
            must_contain: &["Macobs", "Emiac"],
            must_not_contain: &["twenty sixth"],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        // ─── DEVANAGARI ─────────────────────────────────────────────────
        TestCase {
            id: "DR1",
            category: "devanagari",
            description: "Pure Devanagari Hinglish",
            transcript: "hello भाई कैसे हो क्या कर रहे हो",
            must_contain: &["bhai", "kaise"],
            must_not_contain: &["भाई", "कैसे"],
            available_vocab: vec![],
            screen_context: None,
        },
        // ─── COMPLEX ────────────────────────────────────────────────────
        TestCase {
            id: "CX1",
            category: "complex",
            description: "Long message with courtesy preserved",
            transcript: "hello भाई कैसे हो कल शाम की बात हुई थी अपनी कि आप पूरा जो भी है सात बजे तक अपना report दे दो but अभी तक मेरे पास वह आई नहीं है तो please jaldi se bhejo",
            must_contain: &["report", "please"],
            must_not_contain: &["भाई", "कैसे"],
            available_vocab: vec![],
            screen_context: None,
        },
        // ─── SCREEN CONTEXT ─────────────────────────────────────────────
        TestCase {
            id: "SC1",
            category: "screen_context",
            description: "Screen context helps meac→Emiac",
            transcript: "meac में कितना काम हो गया",
            must_contain: &["Emiac"],
            must_not_contain: &["meac", "MEAC"],
            available_vocab: vec![emiac_term()],
            screen_context: Some("Emiac Technologies Pvt Ltd — revenue report Q4"),
        },
        // ─── FILLERS ────────────────────────────────────────────────────
        TestCase {
            id: "FL1",
            category: "fillers",
            description: "English fillers removed",
            transcript: "um okay so basically I was thinking that we should probably go ahead with the plan",
            must_contain: &["plan"],
            must_not_contain: &["basically"],
            available_vocab: vec![],
            screen_context: None,
        },
        // ═══════════════════════════════════════════════════════════════════
        //  DIVERSE PROPER NOUNS — real-world distortions
        // ═══════════════════════════════════════════════════════════════════

        // ─── Perplexity AI (Deepgram splits it) ─────────────────────────
        TestCase {
            id: "DV1",
            category: "diverse_nouns",
            description: "perplex city → Perplexity (common Deepgram split)",
            transcript: "perplex city ai se automation banana hai aur EMIAC mein deploy karna hai",
            must_contain: &["Perplexity"],
            must_not_contain: &["perplex city"],
            available_vocab: vec![perplexity_term(), emiac_term()],
            screen_context: None,
        },
        // ─── Vipassana (classic Deepgram mishearing) ────────────────────
        TestCase {
            id: "DV2",
            category: "diverse_nouns",
            description: "we passed na → Vipassana (meditation retreat)",
            transcript: "yaar main we passed na retreat mein ja raha hoon next month",
            must_contain: &["Vipassana"],
            must_not_contain: &["we passed na"],
            available_vocab: vec![vipassana_term()],
            screen_context: None,
        },
        // ─── Razorpay (Deepgram spaces it) ──────────────────────────────
        TestCase {
            id: "DV3",
            category: "diverse_nouns",
            description: "razor pay → Razorpay (payment gateway)",
            transcript: "razor pay ka integration karna hai payment ke liye app mein",
            must_contain: &["Razorpay"],
            must_not_contain: &["razor pay"],
            available_vocab: vec![razorpay_term()],
            screen_context: None,
        },
        // ─── Kubernetes (completely mangled by Deepgram) ────────────────
        TestCase {
            id: "DV4",
            category: "diverse_nouns",
            description: "cougar netties → Kubernetes (extreme distortion)",
            transcript: "cougar netties cluster pe deploy karna hai production ke liye",
            must_contain: &["Kubernetes"],
            must_not_contain: &["cougar", "netties"],
            available_vocab: vec![kubernetes_term()],
            screen_context: None,
        },
        // ─── Zerodha (Indian broker) ────────────────────────────────────
        TestCase {
            id: "DV5",
            category: "diverse_nouns",
            description: "zero da → Zerodha (stock broker)",
            transcript: "zero da pe stocks buy karna hai reliance ka",
            must_contain: &["Zerodha"],
            must_not_contain: &["zero da"],
            available_vocab: vec![zerodha_term()],
            screen_context: None,
        },
        // ─── Supabase (Deepgram splits it) ──────────────────────────────
        TestCase {
            id: "DV6",
            category: "diverse_nouns",
            description: "super base → Supabase (backend tool)",
            transcript: "super base se backend banana hai instead of firebase",
            must_contain: &["Supabase"],
            must_not_contain: &["super base"],
            available_vocab: vec![supabase_term()],
            screen_context: None,
        },
        // ─── n8n (always misheard) ──────────────────────────────────────
        TestCase {
            id: "DV7",
            category: "diverse_nouns",
            description: "aneten → n8n (workflow tool)",
            transcript: "aneten se automation flow banana hai aur EMIAC mein use karna hai",
            must_contain: &["n8n"],
            must_not_contain: &["aneten", "Aneten"],
            available_vocab: vec![n8n_term(), emiac_term()],
            screen_context: None,
        },
        // ─── Anish (person name, subtle distortion) ─────────────────────
        TestCase {
            id: "DV8",
            category: "diverse_nouns",
            description: "anis → Anish (person name)",
            transcript: "anis ko bolo ki code review kar de jaldi se",
            must_contain: &["Anish"],
            must_not_contain: &["anis"],
            available_vocab: vec![anish_term()],
            screen_context: None,
        },
        // ─── Multi-noun sentence ────────────────────────────────────────
        TestCase {
            id: "DV9",
            category: "diverse_nouns",
            description: "3 proper nouns in one Hinglish sentence",
            transcript: "anis ko bolo ki razor pay integration super base pe karna hai",
            must_contain: &["Anish", "Razorpay", "Supabase"],
            must_not_contain: &["anis", "razor pay", "super base"],
            available_vocab: vec![anish_term(), razorpay_term(), supabase_term()],
            screen_context: None,
        },
        // ─── HOMOPHONE: "finish" must NOT become Anish ──────────────────
        TestCase {
            id: "DV10",
            category: "homophone",
            description: "finish must NOT become Anish",
            transcript: "yaar yeh kaam finish karo jaldi se deadline aa raha hai",
            must_contain: &["finish"],
            must_not_contain: &["Anish"],
            available_vocab: vec![anish_term()],
            screen_context: None,
        },
        // ─── HOMOPHONE: "zero" must NOT become Zerodha ──────────────────
        TestCase {
            id: "DV11",
            category: "homophone",
            description: "zero must NOT become Zerodha",
            transcript: "balance zero ho gaya hai bhai recharge karna padega",
            must_contain: &["zero"],
            must_not_contain: &["Zerodha"],
            available_vocab: vec![zerodha_term()],
            screen_context: None,
        },
        // ═══════════════════════════════════════════════════════════════════
        //  REAL DEEPGRAM OUTPUTS — from actual user testing (May 21 2026)
        //  These are the transcripts Deepgram ACTUALLY produced.
        // ═══════════════════════════════════════════════════════════════════

        // ─── Deepgram joins "yaar" + "miac" into one token ──────────────
        TestCase {
            id: "RD1",
            category: "real_deepgram",
            description: "yarmiac (joined token) → should extract Emiac",
            transcript: "yarmiac और macops दोनों का काम pending है बहुत ज़्यादा",
            must_contain: &["Emiac"],
            must_not_contain: &["yarmiac", "Yarmiac"],
            available_vocab: vec![emiac_term(), macobs_term()],
            screen_context: None,
        },
        // ─── Deepgram outputs mixed Latin+Devanagari ────────────────────
        TestCase {
            id: "RD2",
            category: "real_deepgram",
            description: "meaक (mixed script) → should match Emiac",
            transcript: "meaक technologies में दो हज़ार तक सब complete करना है भाई",
            must_contain: &["Emiac"],
            must_not_contain: &["meaक", "Maine technologies"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
        // ─── Deepgram outputs "macops" (novel distortion of Macobs) ─────
        TestCase {
            id: "RD3",
            category: "real_deepgram",
            description: "macops → should become Macobs",
            transcript: "macops का quarterly report ready hai kya",
            must_contain: &["Macobs"],
            must_not_contain: &["macops"],
            available_vocab: vec![macobs_term()],
            screen_context: None,
        },
        // ─── Deepgram drops "Emiac" entirely, outputs "mia" ─────────────
        TestCase {
            id: "RD4",
            category: "real_deepgram",
            description: "mia → Emiac (very short distortion)",
            transcript: "mia में काम करता हूं",
            must_contain: &["Emiac"],
            must_not_contain: &["mia"],
            available_vocab: vec![emiac_term()],
            screen_context: None,
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
//  MAIN
// ═══════════════════════════════════════════════════════════════════════════

fn main() {
    let api_key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();

    if api_key.is_empty() {
        println!("SKIP: set GROQ_API_KEY or GATEWAY_API_KEY");
        return;
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let client = reqwest::Client::new();
        let cases = build_test_cases();
        let total = cases.len();

        println!("\n{}", "═".repeat(70));
        println!("  SAID PIPELINE QUALITY SUITE — {} tests (WITH selection gates)", total);
        println!("{}\n", "═".repeat(70));

        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut failures: Vec<(String, String, String)> = Vec::new();
        let mut category_stats: std::collections::BTreeMap<String, (usize, usize)> =
            std::collections::BTreeMap::new();

        for (i, tc) in cases.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }

            print!("  [{}/{}] {} — {} ", i + 1, total, tc.id, tc.description);

            // ── STEP 1: Run selection gates (the real pipeline) ──────────
            let selection_results = simulate_selection(&tc.available_vocab, tc.transcript);
            let selected_vocab: Vec<VocabEntry> = selection_results
                .iter()
                .filter(|r| r.selected)
                .map(|r| {
                    tc.available_vocab
                        .iter()
                        .find(|v| v.term == r.term)
                        .unwrap()
                        .to_prompt_entry(VocabResolution::Resolved)
                })
                .collect();

            // Show selection results
            let sel_count = selected_vocab.len();
            let avail_count = tc.available_vocab.len();
            if avail_count > 0 {
                print!("[sel:{sel_count}/{avail_count}] ");
                for sr in &selection_results {
                    if !sr.selected {
                        print!("({} → {})", sr.term, sr.gate_passed);
                    }
                }
            }
            print!("... ");

            // ── STEP 2: Build prompt with ONLY selected vocab ───────────
            let system_prompt =
                build_test_system_prompt(&selected_vocab, tc.screen_context);
            let user_message = build_user_message(tc.transcript, "hinglish");

            // ── STEP 3: Call Groq ───────────────────────────────────────
            let t0 = Instant::now();
            let result = call_groq(&client, &api_key, &system_prompt, &user_message).await;
            let ms = t0.elapsed().as_millis();

            let cat = tc.category.to_string();
            let entry = category_stats.entry(cat).or_insert((0, 0));

            match result {
                Ok(output) => {
                    let mut case_ok = true;
                    let mut fail_reasons = Vec::new();

                    for word in tc.must_contain {
                        let found = output.contains(word)
                            || output.to_lowercase().contains(&word.to_lowercase());
                        if !found {
                            case_ok = false;
                            fail_reasons.push(format!("missing '{word}'"));
                        }
                    }

                    for word in tc.must_not_contain {
                        if output.contains(word) {
                            case_ok = false;
                            fail_reasons.push(format!("contains '{word}'"));
                        }
                    }

                    if case_ok {
                        println!("PASS ({ms}ms)");
                        passed += 1;
                        entry.0 += 1;
                    } else {
                        let reasons = fail_reasons.join(", ");
                        println!("FAIL ({ms}ms)");
                        println!("        output: {:?}", &output[..output.len().min(120)]);
                        println!("        reason: {reasons}");
                        failed += 1;
                        entry.1 += 1;
                        failures.push((
                            tc.id.to_string(),
                            tc.description.to_string(),
                            format!(
                                "vocab_selected={sel_count}/{avail_count} | output={:?} | {reasons}",
                                &output[..output.len().min(80)]
                            ),
                        ));
                    }
                }
                Err(e) => {
                    println!("ERROR: {e}");
                    failed += 1;
                    entry.1 += 1;
                    failures.push((tc.id.to_string(), tc.description.to_string(), e));
                }
            }
        }

        // ── Summary ─────────────────────────────────────────────────────
        println!("\n{}", "═".repeat(70));
        println!(
            "  RESULTS: {passed}/{total} passed ({:.0}%)",
            (passed as f64 / total as f64) * 100.0
        );
        println!("{}", "═".repeat(70));

        println!("\n  BY CATEGORY:");
        for (cat, (p, f)) in &category_stats {
            let t = p + f;
            let pct = (*p as f64 / t as f64) * 100.0;
            let status = if *f == 0 { "✓" } else { "✗" };
            println!("    {status} {cat}: {p}/{t} ({pct:.0}%)");
        }

        if !failures.is_empty() {
            println!("\n  FAILURES:");
            for (id, desc, detail) in &failures {
                println!("    [{id}] {desc}");
                println!("          {detail}");
            }
        }

        let pass_rate = (passed as f64 / total as f64) * 100.0;
        println!(
            "\n  {}",
            if pass_rate >= 98.0 {
                format!("TARGET MET: {pass_rate:.1}% >= 98%")
            } else {
                format!("TARGET NOT MET: {pass_rate:.1}% < 98% — {failed} test(s) to fix")
            }
        );

        if failed > 0 {
            std::process::exit(1);
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════════════

fn make_test_prefs() -> Preferences {
    Preferences {
        user_id: "eval-user".to_string(),
        selected_model: "llama-4-scout-17b-16e-instruct".to_string(),
        tone_preset: "neutral".to_string(),
        custom_prompt: None,
        language: "hi".to_string(),
        output_language: "hinglish".to_string(),
        auto_paste: true,
        edit_capture: true,
        polish_text_hotkey: "".to_string(),
        record_hotkey: "".to_string(),
        learning_enabled: true,
        updated_at: 0,
        gateway_api_key: None,
        deepgram_api_key: None,
        gemini_api_key: None,
        groq_api_key: None,
        cerebras_api_key: None,
        llm_provider: "groq".to_string(),
        stt_provider: "deepgram".to_string(),
    }
}

fn build_test_system_prompt(vocab_entries: &[VocabEntry], screen_context: Option<&str>) -> String {
    let template = default_voice_prompt_template();
    let mut system_prompt =
        render_voice_system_prompt_template(&template, &make_test_prefs(), &[], &[], vocab_entries);

    if let Some(ctx) = screen_context {
        let trimmed: String = ctx.chars().take(500).collect();
        if !trimmed.trim().is_empty() {
            system_prompt.push_str(&format!(
                "\n\nSURROUNDING TEXT (what the user was already writing — background context only):\n\
                 \"{trimmed}\"\n\n\
                 This is what was already in the text field. Use it ONLY to understand the topic. \
                 Do NOT replace correctly transcribed words with words from this text. \
                 Only use it when the transcript contains an obvious STT error (garbled/nonsense word) \
                 AND a word in this surrounding text would make sense in that position. \
                 If the transcript word is a real word — even a different one — keep it as-is.\n"
            ));
        }
    }

    system_prompt
}

#[derive(Deserialize)]
struct GroqResponse {
    choices: Vec<GroqChoice>,
}
#[derive(Deserialize)]
struct GroqChoice {
    message: GroqMessage,
}
#[derive(Deserialize)]
struct GroqMessage {
    content: String,
}

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const GROQ_MODEL: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

async fn call_groq(
    client: &reqwest::Client,
    api_key: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "model": GROQ_MODEL,
        "stream": false,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": 512,
        "stop": ["=== BEGIN TRANSCRIPT", "=== END TRANSCRIPT"],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_message  },
        ]
    });

    let resp = client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Groq {status}: {}",
            &body_text[..body_text.len().min(200)]
        ));
    }

    let groq_resp: GroqResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    groq_resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content.trim().to_string())
        .ok_or_else(|| "empty response".to_string())
}
