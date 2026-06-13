//! Polish a raw STT transcript using the same Groq pipeline as POST /v1/runtime/voice/polish.
//!
//! Usage:
//!   GROQ_API_KEY=... cargo run --bin polish-cli -- "raw transcript here"
//!   echo "raw transcript" | GROQ_API_KEY=... cargo run --bin polish-cli

use said_control_plane::voice_polish_standalone;

#[tokio::main]
async fn main() {
    let groq_key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();
    if groq_key.is_empty() {
        eprintln!("Error: set GROQ_API_KEY or GATEWAY_API_KEY");
        std::process::exit(1);
    }

    let transcript = match std::env::args().nth(1) {
        Some(arg) if arg != "-" => arg,
        _ => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).expect("read stdin");
            buf
        }
    };
    let transcript = transcript.trim();
    if transcript.is_empty() {
        eprintln!("Error: empty transcript");
        std::process::exit(1);
    }

    let output_language = std::env::var("OUTPUT_LANGUAGE").unwrap_or_else(|_| "hinglish".into());
    let selected_model = std::env::var("SELECTED_MODEL").unwrap_or_else(|_| "fast".into());

    match voice_polish_standalone::polish_transcript(
        transcript,
        &output_language,
        &selected_model,
        &groq_key,
        None,
        &[],
    )
    .await
    {
        Ok(polished) => println!("{polished}"),
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(2);
        }
    }
}
