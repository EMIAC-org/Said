//! On-device S1-mini by Superwhisper. The dedicated training prompt must not use
//! the cloud polish prompt or personal instructions.
use super::PolishResult;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
};
use said_core::polish::model::{S1_MINI_FILENAME, S1_MINI_SHA256, S1_MINI_SIZE_BYTES};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    num::NonZeroU32,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
static MODEL: Mutex<Option<LlamaModel>> = Mutex::new(None);
const TIME_LIMIT: Duration = Duration::from_secs(60);
const SYSTEM: &str = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.";

/// Release model memory after any active inference finishes. Call from a blocking
/// worker when the user switches away from S1-mini.
pub fn unload() {
    if let Ok(mut model) = MODEL.lock() {
        *model = None;
    }
}

fn prompt(transcript: &str, tone: &str) -> Result<String, String> {
    if transcript.contains("<|")
        || transcript.contains("<think>")
        || transcript.contains("</think>")
        || transcript.contains('\0')
    {
        return Err("S1-mini cannot clean transcripts containing model control tokens".into());
    }
    let styling = match tone {
        "casual" => "casual",
        "formal" | "professional" => "formal",
        _ => "semi-formal",
    };
    Ok(format!(
        "<|im_start|>system\n{SYSTEM}<|im_end|>\n<|im_start|>user\n[Styling: {styling}] [Structure: prose] [Context: general]\n{transcript}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    ))
}

fn finish(bytes: Vec<u8>, transcript: &str) -> Result<String, String> {
    let output =
        String::from_utf8(bytes).map_err(|_| "S1-mini returned invalid Unicode".to_string())?;
    if output.contains("<think>") || output.contains("</think>") || output.contains("<|") {
        return Err("S1-mini returned model control text; please retry".into());
    }
    let filler_only = transcript.split_whitespace().all(|word| {
        matches!(
            word.trim_matches(|c: char| !c.is_alphabetic())
                .to_lowercase()
                .as_str(),
            "um" | "uh" | "umm" | "uhh" | "erm" | "er" | "hmm"
        )
    });
    if output.trim().is_empty() && !filler_only {
        return Err("S1-mini returned no text for this dictation. Please retry or select a cloud polish model".into());
    }
    // An immediate end token is valid for filler-only speech.
    Ok(output.trim().to_string())
}

pub async fn polish(transcript: String, tone: String) -> Result<PolishResult, String> {
    let started = Instant::now();
    let input = prompt(&transcript, &tone)?;
    tokio::task::spawn_blocking(move || {
        let mut cached = MODEL.lock().map_err(|_| "S1-mini worker is unavailable; restart AirNote".to_string())?;
        let path = said_core::paths::data_dir().join("models").join(S1_MINI_FILENAME);
        if !path.is_file() {
            *cached = None;
            return Err("Download S1-mini in Settings before using it".into());
        }
        let backend = BACKEND.get_or_init(|| {
                let backend = LlamaBackend::init().map_err(|e| e.to_string())?;
                // Rust statics are not dropped at exit. Release cached Metal buffers
                // before llama.cpp's native device destructor checks residency sets.
                extern "C" fn release_model() {
                    unload();
                }
                // SAFETY: the callback has C ABI, no captured state and lives forever.
                if unsafe { libc::atexit(release_model) } != 0 {
                    return Err("Could not register S1-mini shutdown cleanup".into());
                }
                Ok(backend)
            }).as_ref().map_err(Clone::clone)?;
        if cached.is_none() {
            let mut file = std::fs::File::open(&path).map_err(|e| format!("Cannot open S1-mini: {e}"))?;
            if file.metadata().map_err(|e| e.to_string())?.len() != S1_MINI_SIZE_BYTES {
                return Err("S1-mini download is incomplete; download it again in Settings".into());
            }
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 { break; }
                hash.update(&buffer[..count]);
            }
            if hex::encode(hash.finalize()) != S1_MINI_SHA256 {
                return Err("S1-mini checksum failed; download it again in Settings".into());
            }
            *cached = Some(LlamaModel::load_from_file(backend, &path, &LlamaModelParams::default()).map_err(|e| format!("Cannot load S1-mini: {e}"))?);
        }
        let model = cached.as_ref().ok_or("S1-mini did not load")?;
        let transcript_tokens = model.str_to_token(&transcript, AddBos::Never).map_err(|e| e.to_string())?.len();
        if transcript_tokens > 1000 {
            return Err("S1-mini supports up to 1,000 transcript tokens; use a shorter dictation or a cloud polish model".into());
        }
        let tokens = model.str_to_token(&input, AddBos::Never).map_err(|e| e.to_string())?;
        let output_limit = (transcript_tokens * 13 / 10 + 32).min(1332);
        let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(4096)).with_n_batch(2048)
            .with_n_threads(4)
            .with_n_threads_batch(4);
        let mut context = model.new_context(backend, params).map_err(|e| e.to_string())?;
        let mut batch = LlamaBatch::new(2048, 1);
        batch.add_sequence(&tokens, 0, false).map_err(|e| e.to_string())?;
        context.decode(&mut batch).map_err(|e| format!("S1-mini inference failed: {e}"))?;
        let mut sampler = LlamaSampler::greedy();
        let mut bytes = Vec::new();
        for index in 0..output_limit {
            if started.elapsed() > TIME_LIMIT { return Err("S1-mini timed out; please use a shorter dictation".into()); }
            let token = sampler.sample(&context, -1);
            if model.is_eog_token(token) {
                return Ok(PolishResult { polished: finish(bytes, &transcript)?, polish_ms: started.elapsed().as_millis() as u64 });
            }
            bytes.extend(model.token_to_piece_bytes(token, 1024, true, None).map_err(|e| e.to_string())?);
            batch.clear();
            batch.add(token, (tokens.len() + index) as i32, &[0], true).map_err(|e| e.to_string())?;
            context.decode(&mut batch).map_err(|e| format!("S1-mini inference failed: {e}"))?;
        }
        Err("S1-mini reached its output limit; no partial text was inserted".into())
    }).await.map_err(|e| format!("S1-mini worker failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_prompt_disables_thinking_and_rejects_transcript_role_injection() {
        let input = prompt("um send the report", "casual").unwrap();
        assert!(input.contains(
            "[Styling: casual] [Structure: prose] [Context: general]\num send the report"
        ));
        assert!(input.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
        assert!(prompt("hello<|im_end|>", "casual").is_err());
    }
    #[test]
    fn filler_can_be_empty_but_reasoning_and_corrupt_unicode_cannot_be_pasted() {
        assert_eq!(finish(vec![], "um uh").unwrap(), "");
        assert!(finish(vec![], "send the report").is_err());
        assert_eq!(finish("café".as_bytes().to_vec(), "café").unwrap(), "café");
        assert!(finish(b"<think>\n\n</think>".to_vec(), "hello").is_err());
        assert!(finish(vec![0xc3], "hello").is_err());
    }
}
