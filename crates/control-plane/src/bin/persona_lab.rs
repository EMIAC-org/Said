//! Persona A/B lab for the dictation polish prompt.
//!
//! Runs N polish *personas* (different system-prompt framings) over a fixed set
//! of real STT transcripts, through the EXACT control-plane polish pipeline
//! (`voice_polish_standalone::polish_transcript_with_prompt`: number_format →
//! Groq Scout → script guard → literal restore → email recover). Lets us see,
//! side by side, which persona best does *light, sense-making* correction
//! (fix misheard VOCAB names + broken agreement) WITHOUT over-correcting.
//!
//! Usage:
//!   GROQ_API_KEY=... cargo run --bin persona-lab            # all personas, all fixtures
//!   GROQ_API_KEY=... cargo run --bin persona-lab -- P1 P5   # only these persona ids

use said_control_plane::voice_polish_standalone::polish_transcript_with_prompt;
use said_core::polish::prompt::{
    RagExample, VocabEntry, VocabResolution, default_voice_prompt_template,
    render_voice_system_prompt_template,
};
use said_core::polish::types::{Correction, PolishPrefs};

// ── Fixtures: real STT output from the user's two latest recordings ──────────

/// File 1 via the LIVE Swift STT (matches the user's pasted Example 1).
const F1_SWIFT: &str = "Hello bhai, yah to ismen sahi likha hai. Iske aage dekhana hai ki ek itna achchha kaam karta hai. M.E.A.C.A aur main coughs mein yah frequently galti karta hai to usko dekhana hai ki ek itna achchha se likh pa rahe hain.";

/// File 1 via Deepgram nova-3 (Devanagari; clean "M.E.A.C." + "मैकॉफ्स").
const F1_DG: &str = "Hello भाई, ये तो इसने सही लिखा है. इसके आगे देखना कि ये कितना अच्छा काम करता है. M.E.A.C. और मैकॉफ्स में ये फ्रीकेंटली गलती करता है तो उसको देखना है कि ये कितना अच्छा से लिख पा रहा है.";

/// File 2 via Deepgram nova-3 (long EN+HI monologue; manglings: Redis/radius,
/// Supabase/"super base", n8n/"Anitin", GraphQL, observability).
const F2_DG: &str = "Large language models are becoming a foundational layer in modern software system because they can understand messy human language reason across long context and transform raw thoughts into structured output. But their real power becomes much more visible in mixed language environments like English where people naturally switch between English and हिंदी and English. Project names and domain specific phrases in a real world. Workplace, nobody speaks like a clean textbook. A normal sentence may sound like भाई इस API का latency issue check करो. Backend service में भी cache miss हो रही होगी और अगर Redis में fallback आ रहा है तो कैसा होगा काम, है ना? तो the output should become missing the cache and if the radius fallback is failing, compare the traces in the observability dashboard. The task is difficult because English is not just हिंदी plus English, but the backend service may be flow, grammar pronunciation pattern, shortcut filter words, filler words and workplace habits. People say things like deploy हो गया क्या? Client को update पहुंच गया क्या? Anitin workflow trigger super base auth में issue है, GraphQL resolver time out दे रहा है, CI pipeline flaky हो गई है. Strong model has to identify which parts are normal, casuals, speech, which problem becomes even harder in voice system because speech recognition often mishears problem becomes even harder in voice systems. That's why.";

fn fixtures() -> Vec<(&'static str, &'static str)> {
    vec![
        ("F1-swift  (live Swift STT · Example 1)", F1_SWIFT),
        ("F1-dg     (Deepgram · Example 1)", F1_DG),
        ("F2-dg     (Deepgram · LLM monologue)", F2_DG),
    ]
}

/// User's top vocab, injected so VOCAB-aware correction is observable.
fn vocab() -> Vec<VocabEntry> {
    [
        "Emiac", "MACOBS", "Supabase", "n8n", "Redis", "GraphQL", "Deepgram",
    ]
    .iter()
    .map(|t| VocabEntry {
        term: (*t).to_string(),
        context: None,
        resolution: VocabResolution::Resolved,
        term_type: None,
        meaning: None,
        evidence: None,
        stt_aliases: vec![],
    })
    .collect()
}

// ── Round-2 personas: research-informed (short prose, no numbered rule-lists,
// phonetic glossary, leave-alone exemplars), tuned for a weak model at temp 0.2.
// Sources folded in: minimal-edit + pass-through default (arXiv:2405.15216,
// 2506.13148), leave-it-alone few-shot for restraint (2401.07702), glossary
// with phonetic "sounds-like" hints + conditional substitution (2505.17410,
// 2506.07510), no-translate for code-switch (2310.13013), and the Groq
// anti-degeneration guidance (short prose prompt, prose not lists).

/// Phonetic glossary shared by the personas that use one. "Sounds like" hints
/// are the lever that makes a weak model map a misheard span to the right term.
const GLOSSARY: &str = "Known correct terms — replace a transcript word with one of these ONLY if it clearly sounds like it AND fits the context; otherwise leave the word alone:\n\
- Emiac (sounds like \"M.E.A.C\", \"em ee a c\", \"emaic\")\n\
- MACOBS (sounds like \"main coughs\", \"mac obs\", \"maikofs\")\n\
- Supabase (sounds like \"super base\")\n\
- n8n (sounds like \"Anitin\", \"Nitin\", \"n eight n\")\n\
- Redis (sounds like \"radius\", \"Rediff\")\n\
- GraphQL, Deepgram, observability";

/// One-line reminder placed last (instruction-after-input bias + "output once").
const OUTRO: &str = "Output the cleaned text once, in the same Hindi-English mix the speaker used, with no commentary, no quotes, and no repeated words or lines.";

fn personas(filter: &[String]) -> Vec<(&'static str, &'static str, String)> {
    let all: Vec<(&'static str, &'static str, String)> = vec![
        (
            "P0",
            "Baseline (live prompt — control)",
            default_voice_prompt_template(),
        ),
        (
            "N1",
            "Ultra-minimal prose (length-cliff test)",
            format!(
                "You clean a Hinglish (Hindi-English) voice transcript. Keep every word in the language the speaker used — never translate, and keep the Hindi-English mix exactly. Change as few words as possible: fix only clear mishearings, broken grammar (such as a subject and verb that disagree), and names that match the known terms below. If a sentence already makes sense, leave it unchanged.\n\n{GLOSSARY}\n\n{OUTRO}"
            ),
        ),
        (
            "N2",
            "Minimal + leave-alone few-shot",
            format!(
                "You clean a Hinglish voice transcript with the smallest possible edits: fix clear mishearings, broken grammar, and known-term names; never translate; never change tone or meaning; if a sentence already makes sense, leave it unchanged.\n\n{GLOSSARY}\n\n\
Examples (these show the ONLY kinds of change allowed):\n\
Transcript: tum kal se lage hue is par aur koi bhi update nahin de raha hoon\n\
Cleaned: Tum kal se lage huye ho is par, aur koi bhi update nahi de rahe ho.\n\
Transcript: M.E.A.C aur main coughs mein yah frequently galti karta hai\n\
Cleaned: Emiac aur MACOBS mein yah frequently galti karta hai.\n\
Transcript: kuchh bol raha hoon aur yeh kuchh bhi likh raha hai\n\
Cleaned: Kuchh bol raha hoon aur yeh kuchh bhi likh raha hai.\n\n{OUTRO}"
            ),
        ),
        (
            "N3",
            "Glossary-phonetic + coherence",
            format!(
                "You fix a Hinglish voice transcript. Most words are already correct — keep them. Do exactly two jobs: first, when a word clearly sounds like one of the known terms below and fits the context, replace it with the exact known term; second, when a clause does not make sense because the speech was misheard (for example a wrong verb person, like \"tum ... raha hoon\"), make the smallest fix so it reads sensibly. Do not translate, do not change tone, and do not touch words that are already fine.\n\n{GLOSSARY}\n\n{OUTRO}"
            ),
        ),
        (
            "N4",
            "Two-pass self-check (short prose)",
            format!(
                "Clean a Hinglish voice transcript in one careful read. For each sentence, silently ask: does this make sense, and is it what the speaker meant? If yes, keep it exactly. If no, make the minimum change to fix it — usually a misheard word, a subject/verb agreement slip, or a name that matches a known term below. Never translate, never change tone or meaning, and never rewrite a sentence that is already fine.\n\n{GLOSSARY}\n\n{OUTRO}"
            ),
        ),
        (
            "N5",
            "Expert editor + one demo (TAP-collapsed)",
            format!(
                "You are an expert at correcting Hinglish voice dictation. You know speech-to-text often mishears names and verb endings, so you fix those — but you never rewrite what the speaker said correctly, and you never translate between Hindi and English.\n\n{GLOSSARY}\n\n\
Demonstration:\n\
Transcript: M.E.A.C aur main coughs mein yah frequently galti karta tha to dekhana hai ki kitna achchha likh pa raha hai\n\
Cleaned: Emiac aur MACOBS mein yah frequently galti karta tha, to dekhana hai ki kitna achchha likh pa raha hai.\n\n\
Now clean the transcript the same way. {OUTRO}"
            ),
        ),
        (
            "N6",
            "Edit-budget (hard minimal-edit)",
            format!(
                "You clean a Hinglish voice transcript under a strict edit budget: change at most a few words per sentence. The only allowed changes are: fix a clearly misheard word, fix a broken subject/verb agreement, or replace a word that clearly sounds like a known term below with that term. If fixing a sentence would need more than a few word changes, leave that sentence exactly as it is. Never translate, never restyle, and never add or drop ideas.\n\n{GLOSSARY}\n\n{OUTRO}"
            ),
        ),
    ];
    if filter.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|(id, _, _)| filter.iter().any(|f| f.eq_ignore_ascii_case(id)))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();
    if key.is_empty() {
        eprintln!("Error: set GROQ_API_KEY or GATEWAY_API_KEY");
        std::process::exit(1);
    }

    let filter: Vec<String> = std::env::args().skip(1).collect();
    let prefs = PolishPrefs {
        output_language: "hinglish".into(),
        tone_preset: "neutral".into(),
        custom_prompt: None,
    };
    let vocab = vocab();
    let safe_terms: Vec<String> = vocab.iter().map(|v| v.term.clone()).collect();
    let no_rag: &[RagExample] = &[];
    let no_corr: &[Correction] = &[];
    let personas = personas(&filter);

    let mut md = String::from("# Persona polish comparison (Scout model)\n\nVOCAB injected: ");
    md.push_str(&safe_terms.join(", "));
    md.push_str("\n");

    for (flabel, transcript) in fixtures() {
        println!("\n\n################################################################");
        println!("# {flabel}");
        println!("################################################################");
        println!("RAW STT:\n  {transcript}\n");
        md.push_str(&format!("\n## {flabel}\n\n**RAW STT:** {transcript}\n\n"));

        for (pid, plabel, template) in &personas {
            let sys = render_voice_system_prompt_template(
                template,
                &prefs,
                no_rag,
                no_corr,
                &vocab,
                None,
                |_| false,
            );
            match polish_transcript_with_prompt(
                transcript,
                "hinglish",
                "smart",
                &key,
                &sys,
                &safe_terms,
            )
            .await
            {
                Ok(out) => {
                    println!("[{pid}] {plabel}\n      {out}\n");
                    md.push_str(&format!("**[{pid}] {plabel}**\n\n> {out}\n\n"));
                }
                Err(e) => {
                    println!("[{pid}] {plabel}\n      <ERROR: {e}>\n");
                    md.push_str(&format!("**[{pid}] {plabel}** — ERROR: {e}\n\n"));
                }
            }
        }
    }

    if let Err(e) = std::fs::write("../../.context/persona_results.md", &md) {
        eprintln!("(could not write .context/persona_results.md: {e})");
    } else {
        println!("\nSaved markdown -> .context/persona_results.md");
    }
}
