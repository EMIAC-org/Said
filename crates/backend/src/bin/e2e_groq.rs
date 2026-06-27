//! End-to-end Groq LLM test for AirNote's voice-polish pipeline.
//!
//! This binary builds the exact same system prompt the voice route builds
//! (with vocab entries, screen context, etc.) and calls the real Groq API
//! to verify that:
//!   1. A vocab term ("Emiac") is correctly substituted when the transcript
//!      contains a Deepgram distortion ("MEAH").
//!   2. An unrelated word ("MACOBS") is NOT falsely replaced with "Emiac"
//!      even when "Emiac" is in the screen context.
//!
//! Usage:
//!   GROQ_API_KEY=gsk_... cargo run -p said-backend --bin e2e-groq
//!
//! Skips gracefully when no API key is available.

use said_backend::llm::prompt::{
    VocabEntry, VocabResolution, build_user_message, default_voice_prompt_template,
    render_voice_system_prompt_template,
};
use said_backend::store::prefs::Preferences;
use serde::Deserialize;
use std::time::Instant;

fn main() {
    // ── Resolve API key ──────────────────────────────────────────────────────
    let api_key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();

    if api_key.is_empty() {
        println!("SKIP: no GROQ_API_KEY or GATEWAY_API_KEY set — skipping e2e Groq tests");
        return;
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let client = reqwest::Client::new();

        let mut total_pass = 0usize;
        let mut total_fail = 0usize;
        let mut failures: Vec<String> = Vec::new();

        // ═══════════════════════════════════════════════════════════
        //  TEST 1: Vocab correction — "MEAH" should become "Emiac"
        // ═══════════════════════════════════════════════════════════
        println!("\n== TEST 1: Vocab correction — MEAH -> Emiac ==\n");
        {
            let vocab_entries = vec![VocabEntry {
                term: "Emiac".to_string(),
                term_type: Some("proper_noun".to_string()),
                meaning: Some("Indian technology company".to_string()),
                context: Some("Emiac ke naam se karenge hum".to_string()),
                resolution: VocabResolution::Resolved,
                stt_aliases: vec![("meah".to_string(), 2)],
            }];

            let screen_context = "Emiac technology mein kaam karte hain";

            let system_prompt = build_system_prompt_with_screen_context(
                &make_test_prefs(),
                &vocab_entries,
                Some(screen_context),
            );

            let user_message = build_user_message(
                "MEAH ke naam se karenge hum",
                "hinglish",
            );

            println!("  System prompt length: {} chars", system_prompt.len());
            println!("  User message: {:?}", user_message.lines().last().unwrap_or(""));

            let t0 = Instant::now();
            match call_groq(&client, &api_key, &system_prompt, &user_message).await {
                Ok(output) => {
                    let ms = t0.elapsed().as_millis();
                    println!("  Groq response ({ms}ms): {output:?}");

                    let has_emiac = output.contains("Emiac") || output.contains("emiac") || output.contains("EMIAC");
                    check(
                        &mut total_pass,
                        &mut total_fail,
                        &mut failures,
                        "T1: 'MEAH ke naam se karenge hum' -> output contains 'Emiac'",
                        has_emiac,
                        &format!("output={output:?}"),
                    );
                }
                Err(e) => {
                    println!("  ERROR: {e}");
                    total_fail += 1;
                    failures.push(format!("T1: Groq API call failed: {e}"));
                }
            }
        }

        // ═══════════════════════════════════════════════════════════
        //  TEST 2: False positive guard — "MACOBS" must NOT become "Emiac"
        // ═══════════════════════════════════════════════════════════
        println!("\n== TEST 2: False positive guard — MACOBS stays MACOBS ==\n");
        {
            let vocab_entries = vec![VocabEntry {
                term: "Emiac".to_string(),
                term_type: Some("proper_noun".to_string()),
                meaning: Some("Indian technology company".to_string()),
                context: Some("Emiac ke naam se karenge hum".to_string()),
                resolution: VocabResolution::Resolved,
                stt_aliases: vec![("meah".to_string(), 2)],
            }];

            let screen_context = "Emiac technology mein kaam karte hain";

            let system_prompt = build_system_prompt_with_screen_context(
                &make_test_prefs(),
                &vocab_entries,
                Some(screen_context),
            );

            let user_message = build_user_message(
                "MACOBS ka IPO aane wala hai",
                "hinglish",
            );

            println!("  User message: {:?}", user_message.lines().last().unwrap_or(""));

            let t0 = Instant::now();
            match call_groq(&client, &api_key, &system_prompt, &user_message).await {
                Ok(output) => {
                    let ms = t0.elapsed().as_millis();
                    println!("  Groq response ({ms}ms): {output:?}");

                    let has_macobs = output.contains("MACOBS") || output.contains("Macobs") || output.contains("macobs");
                    let has_emiac = output.to_lowercase().contains("emiac");
                    check(
                        &mut total_pass,
                        &mut total_fail,
                        &mut failures,
                        "T2a: 'MACOBS ka IPO aane wala hai' -> output contains 'MACOBS'",
                        has_macobs,
                        &format!("output={output:?}"),
                    );
                    check(
                        &mut total_pass,
                        &mut total_fail,
                        &mut failures,
                        "T2b: 'MACOBS ka IPO aane wala hai' -> output does NOT contain 'Emiac'",
                        !has_emiac,
                        &format!("output={output:?}"),
                    );
                }
                Err(e) => {
                    println!("  ERROR: {e}");
                    total_fail += 1;
                    failures.push(format!("T2: Groq API call failed: {e}"));
                }
            }
        }

        // ═══════════════════════════════════════════════════════════
        //  TEST 3: No vocab, no screen context — plain polish
        // ═══════════════════════════════════════════════════════════
        println!("\n== TEST 3: Plain polish — no vocab, no screen context ==\n");
        {
            let system_prompt = build_system_prompt_with_screen_context(
                &make_test_prefs(),
                &[],
                None,
            );

            let user_message = build_user_message(
                "um okay so basically I was thinking that we should probably go ahead with the plan",
                "english",
            );

            let t0 = Instant::now();
            match call_groq(&client, &api_key, &system_prompt, &user_message).await {
                Ok(output) => {
                    let ms = t0.elapsed().as_millis();
                    println!("  Groq response ({ms}ms): {output:?}");

                    // Should have cleaned up fillers
                    let no_um = !output.to_lowercase().contains(" um ");
                    let no_basically = !output.to_lowercase().contains("basically");
                    let has_plan = output.to_lowercase().contains("plan");
                    check(
                        &mut total_pass,
                        &mut total_fail,
                        &mut failures,
                        "T3a: Fillers removed ('um', 'basically' gone)",
                        no_um && no_basically,
                        &format!("output={output:?}"),
                    );
                    check(
                        &mut total_pass,
                        &mut total_fail,
                        &mut failures,
                        "T3b: Content preserved ('plan' still present)",
                        has_plan,
                        &format!("output={output:?}"),
                    );
                }
                Err(e) => {
                    println!("  ERROR: {e}");
                    total_fail += 1;
                    failures.push(format!("T3: Groq API call failed: {e}"));
                }
            }
        }

        // ═══════════════════════════════════════════════════════════
        //  SUMMARY
        // ═══════════════════════════════════════════════════════════
        println!("\n{}", "=".repeat(60));
        println!(
            "  RESULTS: {} passed, {} failed",
            total_pass, total_fail
        );
        println!("{}", "=".repeat(60));

        if !failures.is_empty() {
            println!("\nFAILURES:\n");
            for f in &failures {
                println!("  FAIL: {f}");
            }
        }

        println!(
            "\n{}",
            if total_fail == 0 { "PASS" } else { "FAIL" }
        );
        if total_fail > 0 {
            std::process::exit(1);
        }
    });
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn check(
    pass: &mut usize,
    fail: &mut usize,
    failures: &mut Vec<String>,
    name: &str,
    ok: bool,
    detail: &str,
) {
    if ok {
        println!("  PASS  {name}");
        *pass += 1;
    } else {
        println!("  FAIL  {name}");
        println!("        {detail}");
        *fail += 1;
        failures.push(format!("{name}: {detail}"));
    }
}

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
        server_runtime_enabled: false,
        server_audio_runtime_enabled: false,
        updated_at: 0,
        gateway_api_key: None,
        deepgram_api_key: None,
        gemini_api_key: None,
        groq_api_key: None,
        cerebras_api_key: None,
        deepinfra_api_key: None,
        llm_provider: "groq".to_string(),
        stt_provider: "deepgram".to_string(),
    }
}

/// Build the full system prompt with optional screen context, exactly mirroring
/// what the voice route does in `polish_with_input`.
fn build_system_prompt_with_screen_context(
    prefs: &Preferences,
    vocab_entries: &[VocabEntry],
    screen_context: Option<&str>,
) -> String {
    let template = default_voice_prompt_template();
    let mut system_prompt = render_voice_system_prompt_template(
        &template,
        prefs,
        &[], // no RAG examples
        &[], // no corrections
        vocab_entries,
    );

    // Append screen context exactly as voice.rs does
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

/// Groq chat completions response (non-streaming).
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

/// Call Groq's non-streaming chat completions endpoint.
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
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>",
        ],
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
        .map_err(|e| format!("Groq request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Groq API error {status}: {}",
            said_core::text::truncate_utf8(&body_text, 400)
        ));
    }

    let groq_resp: GroqResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Groq response: {e}"))?;

    groq_resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content.trim().to_string())
        .ok_or_else(|| "No choices in Groq response".to_string())
}
