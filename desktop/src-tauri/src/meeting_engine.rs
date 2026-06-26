use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufWriter, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, StreamTrait};
use said_recorder::{CHANNELS, SAMPLE_RATE, resample_to_16k};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

trait LockRecoverExt<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockRecoverExt<T> for Mutex<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poison| {
            tracing::warn!("[meeting_engine] recovered poisoned mutex");
            poison.into_inner()
        })
    }
}

const STATUS_EVENT: &str = "meeting-engine-state";
const LIVE_TRANSCRIPT_EVENT: &str = "meeting-engine-live-transcript";
const PHASE: &str = "system_audio_capture";

/// Emit an event from ANY thread without taking the WebView2 webview lock off the
/// UI thread. On Windows, calling `app.emit()` directly from a worker thread (the
/// meeting capture / live-transcript threads) locks the webview cross-thread and
/// can contend with the main thread's IPC handling; marshalling the emit onto the
/// main thread via the event loop avoids that. (Defensive hardening — the primary
/// End-meeting deadlock was the floating pill creating a webview from an IPC
/// handler, wry #583, now disabled on Windows in LiveMeetingView.)
fn emit_main<S>(app: &AppHandle, event: &'static str, payload: S)
where
    S: serde::Serialize + Clone + Send + 'static,
{
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = app2.emit(event, payload);
    });
}
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_WRITER_STOP_POLL: Duration = Duration::from_millis(100);
const AUDIO_QUEUE_DEPTH: usize = 512;
const LIVE_AUDIO_QUEUE_DEPTH: usize = 4096;
// On End Meeting the live worker abandons pending windows promptly (see
// `run_live_transcript_worker`), so it exits within ~one in-flight window. Wait
// long enough to JOIN it cleanly (and release the shared whisper process lock)
// before the authoritative full-file transcription runs — rather than detaching
// a worker that keeps hogging the lock.
const LIVE_TRANSCRIPT_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_LIVE_WHISPER_MAX_CONTEXT_TOKENS: i32 = 224;
const DEFAULT_LIVE_TRANSCRIPT_CONTEXT_SECS: u64 = 30;
const DEFAULT_LIVE_TRANSCRIPT_STEP_SECS: u64 = 30;
const DEFAULT_LIVE_TRANSCRIPT_MIN_SECS: u64 = 2;
const DEFAULT_LIVE_TRANSCRIPT_POLL_MS: u64 = 1_000;
const DEFAULT_LIVE_TRANSCRIPT_TIMEOUT_SECS: u64 = 90;
// Peak ceiling after ASR gain — never push a normalized track past this, so we
// don't clip. Loudness targeting is RMS-based (ASR_TARGET_RMS); this is only the
// anti-clip limiter.
const ASR_TARGET_PEAK: f32 = 0.95;
// Bounded gain. Was 64x: peak-normalizing a near-silent mic (peak 0.019) by ~39x
// amplifies room bleed / noise floor up to speech-like loudness, which sails past
// VAD and the decode thresholds and makes whisper hallucinate (esp. with a forced
// language). A bounded gain recovers genuinely quiet speech without manufacturing
// loudness out of noise; the RMS silence gate (below) drops noise-only tracks
// before they ever reach gain.
const ASR_MAX_GAIN: f32 = 16.0;
const ASR_MIN_PEAK_FOR_GAIN: f32 = 0.002;
// Target loudness (RMS) for ASR normalization, ~ -20 dBFS — conversational
// speech level. Targeting loudness instead of peak preserves SNR and avoids
// blowing up a transient-dominated track's noise floor.
const ASR_TARGET_RMS: f32 = 0.10;
// Speech-energy floor (RMS). A track whose average energy is below this is
// silence / room bleed, not the primary speaker — forcing ASR on it (especially
// with a forced language like hi) only yields hallucinated text. -50 dBFS ≈
// 0.0032 linear. Such tracks are gated to empty instead of transcribed. This is
// the loudness-aware complement to ASR_MIN_PEAK_FOR_TRANSCRIPTION (which only
// looks at a single transient peak and lets bleed spikes through).
const ASR_MIN_RMS_FOR_TRANSCRIPTION: f32 = 0.0032;
// Merged-playback leveling: bring both mic and system tracks toward a common
// target peak so a quiet mic (e.g. peak 0.18) isn't drowned out by a loud
// system track (peak 0.90) in the recording you play back.
const MERGE_MIX_TARGET_PEAK: f32 = 0.6;
const MERGE_MIC_MAX_GAIN: f32 = 12.0;
const SOURCE_ACTIVITY_FRAME_SAMPLES: u64 = SAMPLE_RATE as u64 / 10;
const SOURCE_ACTIVITY_ABSOLUTE_FLOOR: f32 = 0.01;
const SOURCE_ACTIVITY_RELATIVE_FLOOR: f32 = 0.08;
const ECHO_DEDUPE_MAX_START_GAP_MS: u64 = 5_500;
const ECHO_DEDUPE_MAX_INTERVAL_GAP_MS: u64 = 2_500;
const ECHO_DEDUPE_MIN_TEXT_SIMILARITY: f32 = 0.62;
const ECHO_DEDUPE_STRONG_TEXT_SIMILARITY: f32 = 0.82;
const ECHO_DEDUPE_MIN_SYSTEM_COVERAGE: f32 = 0.45;
const ECHO_DEDUPE_MAX_LOCAL_COVERAGE: f32 = 0.35;
const ECHO_DEDUPE_VIDEO_MAX_LOCAL_RATIO: f32 = 0.04;
const ECHO_DEDUPE_VIDEO_MIN_DUPLICATE_RATIO: f32 = 0.25;
const ECHO_DEDUPE_VIDEO_MIN_SYSTEM_MS: u64 = 60_000;
const ECHO_DEDUPE_VIDEO_MIN_SILENCE_COVERAGE: f32 = 0.60;
const WHISPER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DEFAULT_FINAL_ASR_CHUNK_SECS: u64 = 5 * 60;
// Near-silent tracks (e.g. a system track with no remote participant) make
// whisper hallucinate text like "Thank you" / "aaaa" on silence, so require a
// small but real signal before transcribing. -44 dB is still well below even
// whispered speech, so genuine quiet audio is kept.
const ASR_MIN_PEAK_FOR_TRANSCRIPTION: f32 = 0.006;
// Hindi/Hinglish-first meetings: force Hindi for both live chunks and
// after-meeting transcription. The romanizer renders any Devanagari output into
// Roman Hinglish after decoding.
const DEFAULT_WHISPER_LANGUAGE: &str = "hi";
const DEFAULT_WHISPER_MAX_CONTEXT_TOKENS: i32 = 0;
const DEFAULT_WHISPER_SUPPRESS_NON_SPEECH: bool = true;
// Keep temperature fallback ENABLED: it recovers failed decodes and is what the
// raised entropy threshold relies on to escape repetition loops (fallback +
// entropy gate work together — disabling it makes -et a no-op).
const DEFAULT_WHISPER_NO_FALLBACK: bool = false;
const DEFAULT_WHISPER_NO_SPEECH_THRESHOLD: f32 = 0.75;
const DEFAULT_WHISPER_LOGPROB_THRESHOLD: f32 = -0.35;
// 3.0 reduces repetitive hallucination/looping (max is ln 32 ≈ 3.47); validated
// against whisper.cpp maintainer guidance + our own Hindi tests.
const DEFAULT_WHISPER_ENTROPY_THRESHOLD: f32 = 3.0;
// Voice Activity Detection (Silero) — strips silence/non-speech before whisper,
// which is the structural fix for hallucinations on silent mic / speaker bleed.
const DEFAULT_VAD_THRESHOLD: f32 = 0.5;
const DEFAULT_VAD_SPEECH_PAD_MS: i32 = 250;
const DEFAULT_VAD_MIN_SILENCE_MS: i32 = 100;
const DEFAULT_WHISPER_MIN_SEGMENT_CONFIDENCE: f64 = 0.75;
const MEETING_TRANSLITERATE_SYSTEM_PROMPT: &str = r#"You transliterate meeting-transcript lines from Devanagari/Hindi into natural Roman Hinglish.

Input is a numbered list of transcript lines. For EACH line:
- Transliterate Hindi/Devanagari into readable Roman Hinglish (e.g. "मैंने किया" → "maine kiya"). Do NOT translate Hindi into formal English — romanize it.
- Keep English words, names, product/tech terms, and numbers as-is.
- Fix only obvious ASR typos; preserve meaning, order, and length.

Return EXACTLY the same numbered lines, in the same order, with the same count. Output only the numbered lines — no commentary, no extra lines."#;
const GATEWAY_MEETING_CLEANUP_URL: &str = "https://gateway.outreachdeal.com/v1/chat/completions";
const GROQ_MEETING_CLEANUP_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const DEEPSEEK_MEETING_CLEANUP_URL: &str = "https://api.deepseek.com/chat/completions";
const DEFAULT_MEETING_CLEANUP_PROVIDER: &str = "deepseek";
const DEFAULT_GATEWAY_MEETING_CLEANUP_MODEL: &str = "gemini-2.5-flash";
const DEFAULT_GROQ_MEETING_CLEANUP_MODEL: &str = "llama-3.3-70b-versatile";
const DEFAULT_DEEPSEEK_MEETING_CLEANUP_MODEL: &str = "deepseek-v4-pro";
const DEFAULT_MEETING_CLEANUP_TIMEOUT_SECS: u64 = 90;
const DEFAULT_MEETING_CLEANUP_MAX_TOKENS: u64 = 8192;
const DEFAULT_MEETING_AI_TIMEOUT_SECS: u64 = 120;
const DEFAULT_MEETING_AI_MAX_TOKENS: u64 = 8192;
const DEFAULT_MEETING_SPEAKER_NAMING_TIMEOUT_SECS: u64 = 45;
const DEFAULT_MEETING_SPEAKER_NAMING_MAX_TOKENS: u64 = 768;
const DEFAULT_FINAL_DIARIZATION_TIMEOUT_SECS: u64 = 30 * 60;
const MEETING_FINAL_DIARIZATION_MODE_KEY: &str = "AIRNOTE_MEETING_FINAL_DIARIZATION_MODE";
const FINAL_DIARIZATION_MODE_OFF: &str = "off";
const FINAL_DIARIZATION_MODE_LIGHT: &str = "light";
const FINAL_DIARIZATION_MODE_HIGH: &str = "high";
const LIGHT_DIARIZATION_SEGMENTATION_NAME: &str = "segmentation-3.0.onnx";
const LIGHT_DIARIZATION_EMBEDDING_NAME: &str = "wespeaker_en_voxceleb_resnet34_LM.onnx";
const LIGHT_DIARIZATION_SEGMENTATION_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx";
const LIGHT_DIARIZATION_EMBEDDING_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx";
const LIGHT_DIARIZATION_SEGMENTATION_BYTES: u64 = 5_992_913;
const LIGHT_DIARIZATION_EMBEDDING_BYTES: u64 = 26_535_549;
const LIGHT_DIARIZATION_EVENT: &str = "meeting-diarization-model-download";
const LIGHT_DIARIZATION_PROVIDER: &str = "light_sherpa_onnx";
const DEFAULT_LIGHT_DIARIZATION_MAX_SPEAKERS: i32 = 4;
const DEFAULT_LIGHT_DIARIZATION_MAX_AUDIO_SECS: u64 = 15 * 60;
const HIGH_DIARIZATION_PROVIDER: &str = "nemo_sortformer";
const MEETING_CLEANUP_SYSTEM_PROMPT: &str = r#"You are a meeting transcript cleanup engine.

Clean automatic speech recognition output into a readable meeting transcript.

Rules:
- Correct obvious ASR mistakes only when the surrounding context supports the correction.
- Preserve the speaker's meaning, uncertainty, order, and level of detail.
- Output in Roman/Latin script (Hinglish). If any text is in Devanagari/Hindi, transliterate it into natural Roman Hinglish (e.g. "मैंने किया" → "maine kiya"), keeping English words in English. Do NOT translate Hindi into formal English — romanize it as Hinglish.
- Preserve Roman Hinglish when the speaker mixes Hindi and English; do not translate it into formal English.
- Preserve product names, person names, acronyms, model names, version numbers, commands, file names, and technical terms when they are inferable from context.
- Remove non-speech artifacts such as bracketed silence markers.
- Do not invent missing decisions, action items, attendees, dates, or numbers.
- Return only the cleaned transcript text. No markdown, no commentary."#;
const MEETING_SPEAKER_NAMING_SYSTEM_PROMPT: &str = r#"You label meeting transcript speakers.

Your job:
- Infer a human name for an existing speaker_id only when the transcript gives direct evidence.
- Good evidence: the speaker says "this is Anish", another speaker clearly identifies them, or turn-taking makes an addressed name unambiguous.
- Be careful: if speaker_1 says "Rahul, can you...", Rahul is usually the person being addressed, not speaker_1.
- If evidence is weak, ambiguous, or only role-like, omit that speaker_id.
- Do not invent names. Do not output roles like Host, User, Agent, Customer, Speaker, Participant.
- Keep the local user's speaker_id ("you") unnamed unless the transcript clearly identifies that person.

Return JSON only:
{"speakers":[{"speaker_id":"speaker_1","name":"Rahul","evidence":"short reason"}]}"#;
const MEETING_INTELLIGENCE_SYSTEM_PROMPT: &str = r#"You are AirNote's meeting intelligence engine.

Use only the supplied transcript. Do not invent facts, attendees, dates, action items, or decisions.
The transcript may contain speaker labels and timestamps. Preserve uncertainty when the transcript is unclear.
Write the summary field as a detailed, client-ready Minutes of Meeting / MoM, not a short recap.
The MoM must be useful to someone who did not attend: explain the context, what the speakers were trying to do, what got clarified, what changed during the conversation, why it matters, and what remains unresolved.
Connect related points across the meeting when the transcript supports the connection, but do not invent facts beyond the transcript.
The summary field must be clean Markdown-compatible plain text, not HTML. Use numbered section headings and bullets. Do not return one giant paragraph.
Formatting rules for a beautiful, scannable MoM:
- Start each major section with a numbered heading on its own line, e.g. "1. Meeting Context".
- Under each heading, write a short paragraph and/or "- " bullet points. Keep bullets specific and grounded in the transcript.
- For the single most important takeaway of a section, you may put it on its own line prefixed with "> " (a blockquote) — use this sparingly.
- For Decisions, Risks, Action Items, and Next Steps bullets, you may begin the bullet with ONE relevant emoji marker for visual scanning (🤝 alignment/decision, 💭 proposal/idea, ⚠️ risk/caution, 📍 action item, ⚡ next step, ✅ done). At most one emoji per bullet, and only in those sections. Do not scatter emojis through prose or headings.
For short/simple meetings, use fewer sections. For long, technical, sales, client, strategy, product, or operational meetings, make the MoM detailed and structured.
Prefer this numbered MoM structure when supported by the transcript:
1. Meeting Context
2. Participants / Stakeholders and Roles
3. Core Discussion
4. Important Background and Current State
5. Key Questions, Concerns, and Clarifications
6. Explanations / Options Discussed
7. Stakeholder Expectations and Success Criteria
8. Important Decisions or Alignments
9. Risks, Cautions, and Open Points
10. Agreed Action Items
11. Next Steps and Follow-Up Plan
12. Suggested Follow-Up Message
13. Final Interpretation
Do not force empty or irrelevant sections. If a section has no support, omit it or state the uncertainty briefly.
When the transcript is a client, sales, product, project, or consulting discussion, include practical implications where supported, such as client-side expectation, product-side implication, engineering-side implication, agency-side implication, timeline implication, or proposal implication.
Action-style sections in the summary may include tentative follow-ups and open possibilities, but label them as tentative unless the transcript confirms them.
Use specific nouns from the meeting instead of vague phrases like "they discussed various topics".
Summaries may mention proposals, debates, tentative follow-ups, tentative leanings, and unresolved questions.
Action items require an explicit firm commitment, assignment, or follow-up request in the transcript. If ownership is unclear, use null.
Do not include tentative follow-ups like "maybe", "probably", "I might", "we could", or "we can check" as action items. Mention them only in the summary.
Decisions require explicit agreement or a clear final choice. Do not convert brainstorms, preferences, suggestions, or tentative plans into decisions.
Phrases like "maybe", "should", "probably", "I think", "we could", or "we should" are not decisions unless a later turn clearly confirms agreement or commitment. When in doubt, leave decisions empty.
Every action item and decision must include an "evidence" field containing a short exact quote from the transcript line that supports it. If there is no exact quote, omit that item.
If an assignee is non-null, the evidence must clearly support that assignee by name, speaker label, or role. Otherwise set assignee to null.
Every action item must include "support": "firm". Every decision must include "support": "explicit". Omit items that cannot honestly use those support values.

Also produce a "title" and "tags":
- "title": a concise, specific, human-readable meeting heading of 3 to 8 words that names the actual subject (e.g. "Stryker Sentinel Pricing & Rollout", "Nora Email Triage Setup"). No date, no time, no quotes, no trailing period. Never output generic titles like "Quick meeting" or "Meeting notes".
- "tags": 3 to 5 short topic tags (1 to 2 words each, Title Case, no leading '#') capturing the key themes, e.g. ["Pricing", "Security", "Onboarding"]. Use specific nouns grounded in the transcript; do not invent topics that were not discussed.

Return only valid JSON with this exact shape:
{
  "title": "concise specific meeting title, 3-8 words",
  "tags": ["Topic", "Topic", "Topic"],
  "summary": "Markdown-compatible detailed MoM with numbered section headings and bullets where supported",
  "action_items": [
    { "title": "specific action", "assignee": "speaker or person if explicit, else null", "due": "due date if explicit, else null", "evidence": "exact transcript quote", "support": "firm" }
  ],
  "decisions": [
    { "text": "specific decision if explicitly made", "evidence": "exact transcript quote", "support": "explicit" }
  ]
}"#;
const MEETING_INTELLIGENCE_VERIFIER_SYSTEM_PROMPT: &str = r#"You are AirNote's strict meeting intelligence verifier.

Use only the supplied transcript and draft JSON. Return only valid JSON with the same shape as the draft.

Rules:
- Preserve the "title" and "tags" fields. Keep the title concise (3-8 words), specific, and free of dates/times; replace it only if it is generic, inaccurate, or missing. Keep 3-5 grounded Title-Case tags; drop any tag not supported by the transcript.
- Rewrite the summary if it states tentative proposals as settled decisions.
- Preserve or improve the summary's detailed numbered MoM format. Do not collapse it into one paragraph or a short recap.
- The summary should remain useful to someone who did not attend: include supported context, core discussion, key questions, clarifications, implications, risks/open points, action-style follow-ups, and final interpretation where the transcript supports them.
- Remove or soften any unsupported implications, risks, deliverables, follow-up messages, or stakeholder expectations.
- Keep an action item only when the transcript contains an explicit firm commitment, assignment, or follow-up request.
- Remove tentative follow-ups like "maybe", "probably", "I might", "we could", or "we can check" from action items. They can stay in the summary.
- Keep a decision only when the transcript contains explicit agreement or a clear final choice.
- Remove brainstorms, preferences, suggestions, strong leanings, and tentative plans from decisions.
- Every kept action item and decision must include an "evidence" field copied from the transcript. Do not paraphrase evidence.
- Every kept action item must include "support": "firm"; every kept decision must include "support": "explicit".
- If a kept item's evidence does not directly support the item, remove the item.
- If uncertain, remove the action item or decision and mention the uncertainty only in the summary."#;
const MEETING_CHAT_SYSTEM_PROMPT: &str = r#"You are AirNote's meeting Q&A engine.

Answer using only the supplied transcript. Use meeting intelligence as a hint, not as authority.
If the answer is not present, say that the transcript does not contain it.
Do not infer owners, decisions, dates, or commitments beyond the transcript.
When asked about decisions, use only the provided decisions list; if it is empty, say no explicit decisions are captured.
When asked about risks or unresolved questions, do not label an accepted next step as a risk unless the transcript says it is uncertain, blocked, infeasible, or untested.
When writing briefs, keep proposals and leanings out of the Decisions section unless the provided decisions list contains them.
Be concise and cite timestamp/speaker labels when useful."#;

#[derive(Clone, Debug, Serialize)]
pub struct MeetingEngineStatus {
    pub active: bool,
    pub muted: bool,
    pub capture_running: bool,
    pub mic_track_active: bool,
    pub system_track_active: bool,
    pub speaker_reference_available: bool,
    pub echo_gate_active: bool,
    pub local_speech_active: bool,
    pub last_gate_reason: String,
    pub session_id: Option<String>,
    pub started_at_ms: Option<u64>,
    pub generation: u64,
    pub phase: String,
    pub mic_wav_path: Option<String>,
    pub mic_duration_ms: Option<u64>,
    pub mic_samples_written: u64,
    pub mic_dropped_chunks: u64,
    pub system_wav_path: Option<String>,
    pub system_duration_ms: Option<u64>,
    pub system_samples_written: u64,
    pub system_dropped_chunks: u64,
    pub system_capture_status: String,
    pub system_capture_error: Option<String>,
    pub merged_wav_path: Option<String>,
    pub merged_duration_ms: Option<u64>,
    pub merge_status: String,
    pub merge_error: Option<String>,
    pub source_activity_path: Option<String>,
    pub live_transcript_running: bool,
    pub live_transcript_status: String,
    pub live_transcript_provider: Option<String>,
    pub live_transcript_model: Option<String>,
    pub live_transcript_language: Option<String>,
    pub live_transcript_chunk_count: usize,
    pub live_transcript_error: Option<String>,
    pub live_transcript_dropped_audio_chunks: u64,
    pub transcription_running: bool,
    pub transcription_status: String,
    pub transcription_provider: Option<String>,
    pub transcription_model: Option<String>,
    pub transcription_language: Option<String>,
    pub transcription_latency_ms: Option<u64>,
    pub transcript_text_path: Option<String>,
    pub transcript_json_path: Option<String>,
    pub transcript_text: Option<String>,
    pub transcript_cleaned_text: Option<String>,
    pub final_transcript_text: Option<String>,
    pub transcript_cleanup_status: String,
    pub transcript_cleanup_provider: Option<String>,
    pub transcript_cleanup_model: Option<String>,
    pub transcript_cleanup_latency_ms: Option<u64>,
    pub transcript_cleanup_error: Option<String>,
    pub final_diarization_status: String,
    pub final_diarization_provider: Option<String>,
    pub final_diarization_latency_ms: Option<u64>,
    pub final_diarization_json_path: Option<String>,
    pub final_transcript_json_path: Option<String>,
    pub final_diarization_error: Option<String>,
    pub transcription_error: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingProcessingProgress {
    pub stage: String,
    pub current: u64,
    pub total: u64,
    pub label: String,
    pub track: Option<String>,
}

impl MeetingProcessingProgress {
    fn transcribing(current: u64, total: u64, track: MeetingAudioTrack) -> Self {
        let total = total.max(1);
        let current = current.clamp(1, total);
        let label = if total > 1 {
            format!("Transcribing {current}/{total}")
        } else {
            "Transcribing".to_string()
        };
        Self {
            stage: "transcribing".to_string(),
            current,
            total,
            label,
            track: Some(track.source_label().to_string()),
        }
    }
}

#[derive(Clone, Debug)]
struct MeetingSession {
    session_id: String,
    started_at_ms: u64,
    artifact_dir: PathBuf,
    mic_wav_path: PathBuf,
    system_wav_path: PathBuf,
}

#[derive(Clone, Debug)]
struct MicCaptureSummary {
    path: PathBuf,
    samples_written: u64,
    dropped_chunks: u64,
    native_rate: u32,
    duration_ms: u64,
    peak: f32,
}

struct MicCaptureHandle {
    stop_tx: mpsc::Sender<()>,
    done_rx: mpsc::Receiver<Result<MicCaptureSummary, String>>,
    join: Option<JoinHandle<()>>,
}

type SystemCaptureSummary = MicCaptureSummary;
type SystemCaptureHandle = MicCaptureHandle;

struct LiveTranscriptHandle {
    audio_tx: mpsc::SyncSender<LiveAudioChunk>,
    stop_tx: mpsc::Sender<()>,
    done_rx: mpsc::Receiver<()>,
    join: Option<JoinHandle<()>>,
    // Set on End Meeting so the worker abandons pending live windows immediately
    // (mid-drain) and releases the shared whisper process lock for the final pass.
    stop_flag: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct MergedMeetingAudio {
    summary: MicCaptureSummary,
    source_activity_path: PathBuf,
}

#[derive(Clone, Debug)]
struct MeetingTranscriptionPlan {
    mic: MicCaptureSummary,
    system: Option<SystemCaptureSummary>,
    summary: MicCaptureSummary,
    output_paths: TranscriptPaths,
    source_wavs: Vec<PathBuf>,
    source_activity_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct MeetingAudioSnapshot {
    status: String,
    merged_path: Option<PathBuf>,
    source_activity_path: Option<PathBuf>,
    duration_ms: Option<u64>,
    samples_written: u64,
    error: Option<String>,
}

impl Default for MeetingAudioSnapshot {
    fn default() -> Self {
        Self {
            status: "idle".to_string(),
            merged_path: None,
            source_activity_path: None,
            duration_ms: None,
            samples_written: 0,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
struct TranscriptionSnapshot {
    running: bool,
    status: String,
    progress: Option<MeetingProcessingProgress>,
    provider: Option<String>,
    model: Option<String>,
    language: Option<String>,
    latency_ms: Option<u64>,
    text_path: Option<PathBuf>,
    json_path: Option<PathBuf>,
    text: Option<String>,
    cleaned_text: Option<String>,
    final_text: Option<String>,
    cleanup: MeetingCleanupSnapshot,
    final_diarization: MeetingFinalDiarizationSnapshot,
    error: Option<String>,
}

impl Default for TranscriptionSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            status: "idle".to_string(),
            progress: None,
            provider: None,
            model: None,
            language: None,
            latency_ms: None,
            text_path: None,
            json_path: None,
            text: None,
            cleaned_text: None,
            final_text: None,
            cleanup: MeetingCleanupSnapshot::idle(),
            final_diarization: MeetingFinalDiarizationSnapshot::idle(),
            error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingLiveTranscriptChunk {
    chunk_index: u64,
    source: String,
    speaker_id: String,
    speaker_name: String,
    timestamp_ms: u64,
    text: String,
    is_final: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingLiveTranscriptPayload {
    session_id: Option<String>,
    status: String,
    provider: Option<String>,
    model: Option<String>,
    language: Option<String>,
    chunks: Vec<MeetingLiveTranscriptChunk>,
    error: Option<String>,
    dropped_audio_chunks: u64,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingLiveTranscriptEvent {
    session_id: String,
    chunk: MeetingLiveTranscriptChunk,
}

#[derive(Clone, Debug)]
struct LiveTranscriptSnapshot {
    session_id: Option<String>,
    running: bool,
    status: String,
    provider: Option<String>,
    model: Option<String>,
    language: Option<String>,
    chunks: Vec<MeetingLiveTranscriptChunk>,
    error: Option<String>,
    dropped_audio_chunks: u64,
}

impl Default for LiveTranscriptSnapshot {
    fn default() -> Self {
        Self {
            session_id: None,
            running: false,
            status: "idle".to_string(),
            provider: None,
            model: None,
            language: None,
            chunks: Vec::new(),
            error: None,
            dropped_audio_chunks: 0,
        }
    }
}

impl LiveTranscriptSnapshot {
    fn payload(&self) -> MeetingLiveTranscriptPayload {
        MeetingLiveTranscriptPayload {
            session_id: self.session_id.clone(),
            status: self.status.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            language: self.language.clone(),
            chunks: self.chunks.clone(),
            error: self.error.clone(),
            dropped_audio_chunks: self.dropped_audio_chunks,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveAudioSource {
    Mic,
    System,
}

impl LiveAudioSource {
    fn track(self) -> MeetingAudioTrack {
        match self {
            Self::Mic => MeetingAudioTrack::Mic,
            Self::System => MeetingAudioTrack::System,
        }
    }

    fn source_label(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }

    fn speaker_id(self) -> &'static str {
        match self {
            Self::Mic => "you",
            Self::System => "speaker_1",
        }
    }

    fn speaker_name(self) -> &'static str {
        match self {
            Self::Mic => "You",
            Self::System => "Speaker 1",
        }
    }
}

#[derive(Clone, Debug)]
struct LiveAudioChunk {
    source: LiveAudioSource,
    samples: Vec<i16>,
}

#[derive(Clone, Debug)]
struct LiveTranscriptConfig {
    whisper: WhisperCppConfig,
    context_samples: usize,
    step_samples: usize,
    min_samples: usize,
    poll_interval: Duration,
    timeout: Duration,
}

struct LiveTrackBuffer {
    source: LiveAudioSource,
    base_sample: u64,
    next_window_end_sample: u64,
    last_emitted_sample: u64,
    samples: Vec<i16>,
}

impl LiveTrackBuffer {
    fn new(source: LiveAudioSource) -> Self {
        Self {
            source,
            base_sample: 0,
            next_window_end_sample: 0,
            last_emitted_sample: 0,
            samples: Vec::new(),
        }
    }

    fn push(&mut self, samples: Vec<i16>) {
        self.samples.extend(samples);
    }

    fn take_ready_window(
        &mut self,
        context_samples: usize,
        step_samples: usize,
        min_samples: usize,
        force: bool,
    ) -> Option<LiveTranscriptWindow> {
        if self.samples.len() < min_samples {
            return None;
        }
        let available = self.samples.len() as u64;
        let current_end_sample = self.base_sample.saturating_add(available);
        if self.next_window_end_sample == 0 {
            self.next_window_end_sample =
                self.base_sample.saturating_add(step_samples.max(1) as u64);
        }
        if !force && current_end_sample < self.next_window_end_sample {
            return None;
        }

        if force && current_end_sample <= self.last_emitted_sample {
            return None;
        }

        let context_samples = context_samples.max(min_samples).max(1) as u64;
        let start_sample = current_end_sample
            .saturating_sub(context_samples)
            .max(self.base_sample);
        let start_index = start_sample.saturating_sub(self.base_sample) as usize;
        let end_index = current_end_sample.saturating_sub(self.base_sample) as usize;
        let samples = self.samples[start_index..end_index].to_vec();
        let emit_from_sample = self.last_emitted_sample;
        self.last_emitted_sample = current_end_sample;
        self.next_window_end_sample = current_end_sample.saturating_add(step_samples.max(1) as u64);

        let prune_before = current_end_sample
            .saturating_sub(context_samples)
            .max(self.base_sample);
        let prune_count = prune_before.saturating_sub(self.base_sample) as usize;
        if prune_count > 0 {
            self.samples.drain(..prune_count);
            self.base_sample = self.base_sample.saturating_add(prune_count as u64);
        }

        Some(LiveTranscriptWindow {
            source: self.source,
            start_sample,
            emit_from_sample,
            samples,
        })
    }
}

struct LiveTranscriptWindow {
    source: LiveAudioSource,
    start_sample: u64,
    emit_from_sample: u64,
    samples: Vec<i16>,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingCleanupSnapshot {
    status: String,
    provider: Option<String>,
    model: Option<String>,
    latency_ms: Option<u64>,
    error: Option<String>,
}

impl MeetingCleanupSnapshot {
    fn idle() -> Self {
        Self {
            status: "idle".to_string(),
            provider: None,
            model: None,
            latency_ms: None,
            error: None,
        }
    }

    fn running(provider: String, model: String) -> Self {
        Self {
            status: "running".to_string(),
            provider: Some(provider),
            model: Some(model),
            latency_ms: None,
            error: None,
        }
    }

    fn skipped(status: &str, error: impl Into<String>) -> Self {
        Self {
            status: status.to_string(),
            provider: None,
            model: None,
            latency_ms: None,
            error: Some(error.into()),
        }
    }

    fn failed(provider: String, model: String, latency_ms: u64, error: impl Into<String>) -> Self {
        Self {
            status: "failed".to_string(),
            provider: Some(provider),
            model: Some(model),
            latency_ms: Some(latency_ms),
            error: Some(error.into()),
        }
    }

    fn completed(result: &MeetingCleanupResult) -> Self {
        Self {
            status: "completed".to_string(),
            provider: Some(result.provider.clone()),
            model: Some(result.model.clone()),
            latency_ms: Some(result.latency_ms),
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
struct FinalDiarizationPaths {
    diarization_json: PathBuf,
    transcript_json: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingFinalDiarizationSnapshot {
    status: String,
    provider: Option<String>,
    latency_ms: Option<u64>,
    diarization_json_path: Option<PathBuf>,
    transcript_json_path: Option<PathBuf>,
    error: Option<String>,
}

impl MeetingFinalDiarizationSnapshot {
    fn idle() -> Self {
        Self {
            status: "idle".to_string(),
            provider: None,
            latency_ms: None,
            diarization_json_path: None,
            transcript_json_path: None,
            error: None,
        }
    }

    fn skipped(
        status: &str,
        error: impl Into<String>,
        paths: Option<FinalDiarizationPaths>,
    ) -> Self {
        Self {
            status: status.to_string(),
            provider: None,
            latency_ms: None,
            diarization_json_path: paths.as_ref().map(|paths| paths.diarization_json.clone()),
            transcript_json_path: paths.map(|paths| paths.transcript_json),
            error: Some(error.into()),
        }
    }

    fn running(provider: String, paths: &FinalDiarizationPaths) -> Self {
        Self {
            status: "running".to_string(),
            provider: Some(provider),
            latency_ms: None,
            diarization_json_path: Some(paths.diarization_json.clone()),
            transcript_json_path: Some(paths.transcript_json.clone()),
            error: None,
        }
    }

    fn completed(provider: String, latency_ms: u64, paths: &FinalDiarizationPaths) -> Self {
        Self {
            status: "completed".to_string(),
            provider: Some(provider),
            latency_ms: Some(latency_ms),
            diarization_json_path: Some(paths.diarization_json.clone()),
            transcript_json_path: Some(paths.transcript_json.clone()),
            error: None,
        }
    }

    fn failed(
        provider: String,
        latency_ms: u64,
        paths: &FinalDiarizationPaths,
        error: impl Into<String>,
    ) -> Self {
        Self {
            status: "failed".to_string(),
            provider: Some(provider),
            latency_ms: Some(latency_ms),
            diarization_json_path: Some(paths.diarization_json.clone()),
            transcript_json_path: Some(paths.transcript_json.clone()),
            error: Some(error.into()),
        }
    }
}

#[derive(Clone, Debug)]
struct WhisperCppConfig {
    binary: PathBuf,
    model: PathBuf,
    language: String,
    max_context_tokens: i32,
    prompt: Option<String>,
    suppress_non_speech: bool,
    no_fallback: bool,
    no_speech_threshold: Option<f32>,
    logprob_threshold: Option<f32>,
    entropy_threshold: Option<f32>,
    min_segment_confidence: Option<f64>,
    // Silero VAD model path (None disables VAD) + tuning. When set, whisper only
    // transcribes detected speech segments.
    vad_model: Option<PathBuf>,
    vad_threshold: f32,
    vad_speech_pad_ms: i32,
    vad_min_silence_ms: i32,
    // Romanize Devanagari output into Roman Hinglish (no-op on Latin text).
    romanize: bool,
}

#[derive(Clone, Debug)]
struct TranscriptPaths {
    text: PathBuf,
    json: PathBuf,
    whisper_out_base: PathBuf,
    whisper_txt: PathBuf,
    whisper_json: PathBuf,
}

#[derive(Clone, Debug)]
struct WhisperTranscriptionDone {
    transcript: String,
    latency_ms: u64,
    segments: Vec<RawTranscriptSegment>,
}

#[derive(Clone, Debug)]
struct WhisperAudioChunk {
    summary: MicCaptureSummary,
    start_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct WhisperChunkProgress {
    track: MeetingAudioTrack,
    current: u64,
    total: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeetingAudioTrack {
    Mic,
    System,
}

impl MeetingAudioTrack {
    fn source_label(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Debug)]
struct MeetingPlanTranscriptionDone {
    transcript: String,
    latency_ms: u64,
    summary: MicCaptureSummary,
    segments: Vec<MeetingTranscriptSegment>,
    source_wavs: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct RawTranscriptSegment {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone, Debug)]
struct WhisperSegmentCandidate {
    segment: RawTranscriptSegment,
    confidence: Option<f64>,
    repeated_run: bool,
}

#[derive(Clone, Debug)]
struct MeetingCleanupResult {
    transcript: String,
    provider: String,
    model: String,
    latency_ms: u64,
}

#[derive(Clone, Debug)]
struct MeetingCleanupConfig {
    provider: String,
    url: String,
    auth_header_name: String,
    auth_header_value: String,
    model: String,
}

#[derive(Clone, Debug)]
struct MeetingLlmCompletion {
    content: String,
    provider: String,
    model: String,
    latency_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
struct MeetingSpeakerNamingPayload {
    #[serde(default)]
    speakers: Vec<MeetingSpeakerNamingItem>,
}

#[derive(Debug, Default, Deserialize)]
struct MeetingSpeakerNamingItem {
    #[serde(default)]
    speaker_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    evidence: String,
}

#[derive(Clone, Debug)]
struct MeetingAiTranscript {
    source: String,
    text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeetingAiActionItem {
    title: String,
    assignee: Option<String>,
    due: Option<String>,
    evidence: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeetingAiDecision {
    text: String,
    evidence: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeetingIntelligenceResult {
    status: String,
    provider: String,
    model: String,
    latency_ms: u64,
    transcript_source: String,
    // AI-generated meeting title and topic tags. `default` keeps older cache
    // files (written before these existed) deserializable.
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    summary: String,
    action_items: Vec<MeetingAiActionItem>,
    decisions: Vec<MeetingAiDecision>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingChatResult {
    status: String,
    provider: String,
    model: String,
    latency_ms: u64,
    transcript_source: String,
    answer: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingCachedTranscriptSegment {
    source: String,
    speaker_id: String,
    speaker_name: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MeetingCachedArtifacts {
    meeting_id: Option<String>,
    artifact_dir: String,
    audio_path: Option<String>,
    audio_duration_ms: Option<u64>,
    transcript_path: Option<String>,
    transcript_source: String,
    transcript: String,
    segments: Vec<MeetingCachedTranscriptSegment>,
}

#[derive(Clone, Debug)]
struct MeetingFinalDiarizationConfig {
    provider: String,
    command: PathBuf,
    script: Option<PathBuf>,
    timeout: Duration,
}

#[derive(Clone, Debug)]
enum MeetingFinalDiarizationRunner {
    LightOnnx,
    Command(MeetingFinalDiarizationConfig),
}

impl MeetingFinalDiarizationRunner {
    fn provider(&self) -> String {
        match self {
            Self::LightOnnx => LIGHT_DIARIZATION_PROVIDER.to_string(),
            Self::Command(config) => config.provider.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MeetingTranscriptSegment {
    source: String,
    speaker_id: String,
    speaker_name: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MeetingTranscriptArtifact {
    schema_version: u8,
    provider: String,
    status: String,
    language: Option<String>,
    model: Option<String>,
    source_wav: String,
    source_wavs: Vec<String>,
    diarization_json_path: Option<String>,
    final_diarization_json_path: Option<String>,
    final_transcript_json_path: Option<String>,
    transcript: String,
    cleaned_transcript: Option<String>,
    cleanup_status: String,
    cleanup_provider: Option<String>,
    cleanup_model: Option<String>,
    cleanup_latency_ms: Option<u64>,
    cleanup_error: Option<String>,
    segments: Vec<MeetingTranscriptSegment>,
    audio_duration_ms: u64,
    samples_written: u64,
    latency_ms: Option<u64>,
    generated_at_ms: u64,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingDiarizationArtifact {
    schema_version: u8,
    status: String,
    method: String,
    speakers: Vec<MeetingDiarizationSpeaker>,
    segments: Vec<MeetingDiarizationSegment>,
    generated_at_ms: u64,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingDiarizationSpeaker {
    speaker_id: String,
    speaker_name: String,
    source: String,
    role: String,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingDiarizationSegment {
    speaker_id: String,
    speaker_name: String,
    source: String,
    start_ms: u64,
    end_ms: u64,
    confidence: f32,
    method: String,
}

#[derive(Clone, Debug)]
struct LightDiarizationTurn {
    speaker_key: String,
    start_ms: u64,
    end_ms: u64,
    confidence: f32,
}

#[derive(Clone, Debug)]
struct SourceActivityFrame {
    start_sample: u64,
    end_sample: u64,
    mic_rms: f32,
    system_rms: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SourceActivitySegment {
    source: String,
    start_ms: u64,
    end_ms: u64,
    mic_rms: f32,
    system_rms: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MeetingAudioArtifact {
    schema_version: u8,
    status: String,
    mic_wav: String,
    system_wav: String,
    merged_wav: Option<String>,
    source_activity_path: Option<String>,
    sample_rate: u32,
    channels: u16,
    duration_ms: Option<u64>,
    samples_written: u64,
    source_activity_segments: Vec<SourceActivitySegment>,
    generated_at_ms: u64,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct SegmentActivityCoverage {
    covered_ms: u64,
    local_mic_ms: u64,
    system_audio_ms: u64,
    overlap_ms: u64,
    silence_ms: u64,
}

impl SegmentActivityCoverage {
    fn system_active_ms(self) -> u64 {
        self.system_audio_ms + self.overlap_ms
    }

    fn system_active_ratio(self, duration_ms: u64) -> f32 {
        ratio_ms(self.system_active_ms(), duration_ms)
    }

    fn local_mic_ratio(self, duration_ms: u64) -> f32 {
        ratio_ms(self.local_mic_ms, duration_ms)
    }

    fn silence_ratio(self, duration_ms: u64) -> f32 {
        ratio_ms(self.silence_ms, duration_ms)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceActivitySummary {
    local_mic_ms: u64,
    system_audio_ms: u64,
    overlap_ms: u64,
}

impl SourceActivitySummary {
    fn system_active_ms(self) -> u64 {
        self.system_audio_ms + self.overlap_ms
    }

    fn local_ratio(self) -> f32 {
        let active_ms = self.local_mic_ms + self.system_active_ms();
        ratio_ms(self.local_mic_ms, active_ms)
    }
}

#[derive(Clone, Copy, Debug)]
struct EchoMatch {
    similarity: f32,
    start_gap_ms: u64,
    interval_gap_ms: u64,
}

pub struct MeetingEngineState {
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    session: Mutex<Option<MeetingSession>>,
    mic: Mutex<Option<MicCaptureHandle>>,
    system: Mutex<Option<SystemCaptureHandle>>,
    live_transcript: Arc<Mutex<LiveTranscriptSnapshot>>,
    live_transcript_handle: Mutex<Option<LiveTranscriptHandle>>,
    last_mic_summary: Mutex<Option<MicCaptureSummary>>,
    last_system_summary: Mutex<Option<SystemCaptureSummary>>,
    audio: Mutex<MeetingAudioSnapshot>,
    transcription: Arc<Mutex<TranscriptionSnapshot>>,
    last_error: Mutex<Option<String>>,
    system_error: Mutex<Option<String>>,
    // Per-meeting processing queue drained by a single background worker. This
    // is what makes ending meeting A (still transcribing) then immediately
    // starting + ending meeting B safe: B is queued, not dropped. Also owns the
    // retry/backoff policy and the dedup guard for regenerate requests.
    jobs: Arc<MeetingJobQueue>,
    // Serializes End. The End flow fires a request-stop EVENT *and* a stop INVOKE
    // (belt-and-suspenders on Windows), so stop() can run 2-3x concurrently.
    // Without this, concurrent calls race stop_{system,mic}_capture and split the
    // two track summaries across separate calls — producing a mic-only plan that
    // orphans the captured system track (the meeting then fails with "no
    // confident speech" even though system.wav holds good audio). With the lock,
    // exactly one call captures both tracks + builds the plan; the rest run after
    // it, find nothing left, and no-op.
    stop_lock: Mutex<()>,
}

impl Default for MeetingEngineState {
    fn default() -> Self {
        Self::new()
    }
}

impl MeetingEngineState {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            session: Mutex::new(None),
            mic: Mutex::new(None),
            system: Mutex::new(None),
            live_transcript: Arc::new(Mutex::new(LiveTranscriptSnapshot::default())),
            live_transcript_handle: Mutex::new(None),
            last_mic_summary: Mutex::new(None),
            last_system_summary: Mutex::new(None),
            audio: Mutex::new(MeetingAudioSnapshot::default()),
            transcription: Arc::new(Mutex::new(TranscriptionSnapshot::default())),
            last_error: Mutex::new(None),
            system_error: Mutex::new(None),
            jobs: Arc::new(MeetingJobQueue::new()),
            stop_lock: Mutex::new(()),
        }
    }

    fn start(&self, meeting_id: Option<String>, app: Option<AppHandle>) -> MeetingEngineStatus {
        self.start_session_with_app(true, meeting_id, app)
    }

    fn start_session(&self, enable_mic_capture: bool) -> MeetingEngineStatus {
        self.start_session_with_app(enable_mic_capture, None, None)
    }

    fn start_session_with_app(
        &self,
        enable_mic_capture: bool,
        meeting_id: Option<String>,
        app: Option<AppHandle>,
    ) -> MeetingEngineStatus {
        self.muted.store(false, Ordering::SeqCst);

        let mut session = self.session.lock_recover();
        // Guard: if a DIFFERENT meeting is already active, finalize it before
        // starting the new one. Otherwise the init block below is skipped (it
        // only runs when `session.is_none()`), capture is silently re-armed, and
        // the new recording appends to the previous meeting's folder under the
        // wrong id — direct data misattribution. Same-id re-entry is left as an
        // idempotent re-arm.
        if let Some(existing) = session.as_ref() {
            // Only finalize-and-restart when a DIFFERENT explicit meeting id is
            // requested. A no-id start (or the same id) is an idempotent re-arm
            // of the current session — never silently abandons it.
            if let Some(requested) = safe_meeting_dir_id(meeting_id.as_deref()) {
                if requested != existing.session_id {
                    let previous = existing.session_id.clone();
                    drop(session);
                    tracing::warn!(
                        previous = %previous,
                        requested = %requested,
                        "[meeting_engine] start_session called for a different meeting while one is active; stopping the previous meeting first"
                    );
                    self.stop();
                    session = self.session.lock_recover();
                }
            }
        }
        if session.is_none() {
            self.stop_live_transcript();
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let started_at_ms = now_ms();
            // Prefer the caller's meeting id so artifacts land in the meeting's
            // own folder and resolve by id later. Falls back to a generated id
            // when none is supplied or it is not a safe path component.
            let session_id = safe_meeting_dir_id(meeting_id.as_deref())
                .unwrap_or_else(|| format!("local-{started_at_ms}-{generation}"));
            let live_session_id = session_id.clone();
            let artifact_dir = said_core::paths::data_dir()
                .join("meetings")
                .join(&session_id);
            let mic_wav_path = artifact_dir.join("mic.wav");
            let system_wav_path = artifact_dir.join("system.wav");
            if let Err(e) = fs::create_dir_all(&artifact_dir) {
                self.set_last_error(Some(format!(
                    "failed to create meeting artifact directory: {e}"
                )));
            } else {
                self.set_last_error(None);
                // Checkpoint: a crash from here until transcription leaves a
                // "recording" marker that startup recovery can act on.
                write_meeting_state(&artifact_dir, MEETING_PHASE_RECORDING, None);
            }
            *session = Some(MeetingSession {
                session_id,
                started_at_ms,
                artifact_dir,
                mic_wav_path,
                system_wav_path,
            });
            *self.last_mic_summary.lock_recover() = None;
            *self.last_system_summary.lock_recover() = None;
            *self.system_error.lock_recover() = None;
            *self.audio.lock_recover() = MeetingAudioSnapshot::default();
            *self.transcription.lock_recover() = TranscriptionSnapshot::default();
            *self.live_transcript.lock_recover() = LiveTranscriptSnapshot {
                session_id: Some(live_session_id),
                ..LiveTranscriptSnapshot::default()
            };
        }
        self.active.store(true, Ordering::SeqCst);
        drop(session);

        if enable_mic_capture {
            self.ensure_live_transcript(app);
            self.ensure_mic_capture();
            self.ensure_system_capture();
        }

        self.status()
    }

    fn stop(&self) -> MeetingEngineStatus {
        // Serialize concurrent End calls so one stop captures BOTH track summaries
        // and builds the plan atomically (see `stop_lock`). Subsequent calls run
        // after this returns, find mic/system/session already taken, and no-op —
        // instead of racing and orphaning the system track into a mic-only plan.
        let _stop_guard = self.stop_lock.lock_recover();
        let session = self.session.lock_recover().clone();
        let system_summary = self.stop_system_capture();
        let mic_summary = self.stop_mic_capture();
        self.stop_live_transcript();
        if self.mic.lock_recover().is_some() || self.system.lock_recover().is_some() {
            self.active.store(true, Ordering::SeqCst);
            self.muted.store(false, Ordering::SeqCst);
            return self.status();
        }
        // Churn/abort guard (see MEETING_MIN_SESSION_MS): a stop that lands within
        // the startup window discards the fragment without transcription, so a
        // spurious double start→stop→start never transcribes a sub-second clip
        // and marks the meeting FAILED — which would block recovery of the real
        // recording that immediately follows.
        if let Some(active_session) = session.as_ref() {
            let age_ms = now_ms().saturating_sub(active_session.started_at_ms);
            if age_ms < MEETING_MIN_SESSION_MS {
                let dir = active_session.artifact_dir.clone();
                self.active.store(false, Ordering::SeqCst);
                self.muted.store(false, Ordering::SeqCst);
                self.generation.fetch_add(1, Ordering::SeqCst);
                *self.session.lock_recover() = None;
                cleanup_empty_session_dir(&dir);
                tracing::info!(
                    age_ms,
                    "[meeting_engine] stop within startup window — discarding fragment without transcription"
                );
                return self.status();
            }
        }
        let transcription_plan = self.prepare_transcription_source(
            session.as_ref(),
            mic_summary.clone(),
            system_summary,
        );
        let session_dir = session.as_ref().map(|s| s.artifact_dir.clone());
        self.active.store(false, Ordering::SeqCst);
        self.muted.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut session_guard = self.session.lock_recover();
        *session_guard = None;
        drop(session_guard);

        if let Some(plan) = transcription_plan {
            self.start_transcription_job(plan);
        } else if let Some(dir) = session_dir {
            // No audio was captured (immediate stop / denied mic / silence) — the
            // start created an empty placeholder dir. Remove it so it doesn't
            // accumulate as an invisible orphan forever.
            cleanup_empty_session_dir(&dir);
        }

        self.status()
    }

    fn toggle_mute(&self) -> MeetingEngineStatus {
        if self.active.load(Ordering::SeqCst) {
            self.muted.fetch_xor(true, Ordering::SeqCst);
            self.generation.fetch_add(1, Ordering::SeqCst);
        }

        self.status()
    }

    fn status(&self) -> MeetingEngineStatus {
        let active = self.active.load(Ordering::SeqCst);
        let muted = self.muted.load(Ordering::SeqCst);
        let generation = self.generation.load(Ordering::SeqCst);
        let session = self.session.lock_recover().clone();
        let mic_running = self.mic.lock_recover().is_some();
        let system_running = self.system.lock_recover().is_some();
        let summary = self.last_mic_summary.lock_recover().clone();
        let system_summary = self.last_system_summary.lock_recover().clone();
        let last_error = self.last_error.lock_recover().clone();
        let system_error = self.system_error.lock_recover().clone();
        let transcription = self.transcription.lock_recover().clone();
        let live_transcript = self.live_transcript.lock_recover().clone();
        let audio = self.audio.lock_recover().clone();

        let capture_running = active && !muted && (mic_running || system_running);
        let mic_wav_path = session
            .as_ref()
            .map(|session| session.mic_wav_path.clone())
            .or_else(|| summary.as_ref().map(|summary| summary.path.clone()))
            .map(|path| path.to_string_lossy().to_string());
        let mic_duration_ms = summary.as_ref().map(|summary| summary.duration_ms);
        let mic_samples_written = summary
            .as_ref()
            .map(|summary| summary.samples_written)
            .unwrap_or_default();
        let mic_dropped_chunks = summary
            .as_ref()
            .map(|summary| summary.dropped_chunks)
            .unwrap_or_default();
        let system_wav_path = session
            .as_ref()
            .map(|session| session.system_wav_path.clone())
            .or_else(|| system_summary.as_ref().map(|summary| summary.path.clone()))
            .map(|path| path.to_string_lossy().to_string());
        let system_duration_ms = system_summary.as_ref().map(|summary| summary.duration_ms);
        let system_samples_written = system_summary
            .as_ref()
            .map(|summary| summary.samples_written)
            .unwrap_or_default();
        let system_dropped_chunks = system_summary
            .as_ref()
            .map(|summary| summary.dropped_chunks)
            .unwrap_or_default();
        let system_capture_status =
            system_capture_status(active, system_running, &system_summary, &system_error);

        MeetingEngineStatus {
            active,
            muted,
            capture_running,
            mic_track_active: mic_running,
            system_track_active: system_running,
            speaker_reference_available: false,
            echo_gate_active: false,
            local_speech_active: false,
            last_gate_reason: status_reason(
                active,
                muted,
                mic_running,
                system_running,
                last_error.as_deref(),
            ),
            session_id: session.as_ref().map(|session| session.session_id.clone()),
            started_at_ms: session.as_ref().map(|session| session.started_at_ms),
            generation,
            phase: PHASE.to_string(),
            mic_wav_path,
            mic_duration_ms,
            mic_samples_written,
            mic_dropped_chunks,
            system_wav_path,
            system_duration_ms,
            system_samples_written,
            system_dropped_chunks,
            system_capture_status,
            system_capture_error: system_error,
            merged_wav_path: audio
                .merged_path
                .map(|path| path.to_string_lossy().to_string()),
            merged_duration_ms: audio.duration_ms,
            merge_status: audio.status,
            merge_error: audio.error,
            source_activity_path: audio
                .source_activity_path
                .map(|path| path.to_string_lossy().to_string()),
            live_transcript_running: live_transcript.running,
            live_transcript_status: live_transcript.status,
            live_transcript_provider: live_transcript.provider,
            live_transcript_model: live_transcript.model,
            live_transcript_language: live_transcript.language,
            live_transcript_chunk_count: live_transcript.chunks.len(),
            live_transcript_error: live_transcript.error,
            live_transcript_dropped_audio_chunks: live_transcript.dropped_audio_chunks,
            transcription_running: transcription.running,
            transcription_status: transcription.status,
            transcription_provider: transcription.provider,
            transcription_model: transcription.model,
            transcription_language: transcription.language,
            transcription_latency_ms: transcription.latency_ms,
            transcript_text_path: transcription
                .text_path
                .map(|path| path.to_string_lossy().to_string()),
            transcript_json_path: transcription
                .json_path
                .map(|path| path.to_string_lossy().to_string()),
            transcript_text: transcription.text,
            transcript_cleaned_text: transcription.cleaned_text,
            final_transcript_text: transcription.final_text,
            transcript_cleanup_status: transcription.cleanup.status,
            transcript_cleanup_provider: transcription.cleanup.provider,
            transcript_cleanup_model: transcription.cleanup.model,
            transcript_cleanup_latency_ms: transcription.cleanup.latency_ms,
            transcript_cleanup_error: transcription.cleanup.error,
            final_diarization_status: transcription.final_diarization.status,
            final_diarization_provider: transcription.final_diarization.provider,
            final_diarization_latency_ms: transcription.final_diarization.latency_ms,
            final_diarization_json_path: transcription
                .final_diarization
                .diarization_json_path
                .map(|path| path.to_string_lossy().to_string()),
            final_transcript_json_path: transcription
                .final_diarization
                .transcript_json_path
                .map(|path| path.to_string_lossy().to_string()),
            final_diarization_error: transcription.final_diarization.error,
            transcription_error: transcription.error,
            last_error,
        }
    }

    fn ensure_live_transcript(&self, app: Option<AppHandle>) {
        if self.live_transcript_handle.lock_recover().is_some() {
            return;
        }

        let session = self.session.lock_recover().clone();
        let Some(session) = session else {
            self.set_live_transcript_error("meeting session is not initialized".to_string());
            return;
        };

        if !env_bool("AIRNOTE_MEETING_LIVE_TRANSCRIPT_ENABLED", true) {
            let mut live = self.live_transcript.lock_recover();
            live.session_id = Some(session.session_id);
            live.running = false;
            live.status = "disabled".to_string();
            live.error = None;
            return;
        }

        let config = match resolve_live_transcript_config() {
            Ok(config) => config,
            Err(e) => {
                self.set_live_transcript_error(e);
                return;
            }
        };

        match start_live_transcript_worker(session, config, Arc::clone(&self.live_transcript), app)
        {
            Ok(handle) => {
                *self.live_transcript_handle.lock_recover() = Some(handle);
            }
            Err(e) => {
                self.set_live_transcript_error(e);
            }
        }
    }

    fn stop_live_transcript(&self) {
        let handle = self.live_transcript_handle.lock_recover().take();
        let Some(mut handle) = handle else {
            return;
        };

        // Signal stop two ways: the flag is checked mid-drain (abandons pending
        // windows instantly), and dropping audio_tx wakes the worker from its
        // recv. Together the worker exits within ~one in-flight window, releasing
        // the whisper lock before the authoritative full-file pass runs.
        handle.stop_flag.store(true, Ordering::Relaxed);
        let _ = handle.stop_tx.send(());
        drop(handle.audio_tx);
        match handle.done_rx.recv_timeout(LIVE_TRANSCRIPT_STOP_TIMEOUT) {
            Ok(()) => {
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
            }
            Err(_) => {
                tracing::warn!(
                    "[meeting_engine] live transcript worker did not stop within timeout; detaching"
                );
            }
        }
    }

    fn live_transcript_payload(&self) -> MeetingLiveTranscriptPayload {
        self.live_transcript.lock_recover().payload()
    }

    fn live_audio_sender(&self) -> Option<mpsc::SyncSender<LiveAudioChunk>> {
        self.live_transcript_handle
            .lock_recover()
            .as_ref()
            .map(|handle| handle.audio_tx.clone())
    }

    fn set_live_transcript_error(&self, error: String) {
        tracing::warn!(error = %error, "[meeting_engine] live transcript unavailable");
        let mut live = self.live_transcript.lock_recover();
        live.running = false;
        live.status = "skipped".to_string();
        live.error = Some(error);
    }

    fn ensure_mic_capture(&self) {
        if self.mic.lock_recover().is_some() {
            return;
        }

        let session = self.session.lock_recover().clone();
        let Some(session) = session else {
            self.set_last_error(Some("meeting session is not initialized".to_string()));
            return;
        };

        tracing::info!(
            session_id = %session.session_id,
            artifact_dir = %session.artifact_dir.display(),
            mic_wav_path = %session.mic_wav_path.display(),
            "[meeting_engine] starting mic capture"
        );

        match start_mic_capture(
            session.mic_wav_path,
            Arc::clone(&self.muted),
            self.live_audio_sender(),
        ) {
            Ok(handle) => {
                *self.mic.lock_recover() = Some(handle);
                self.set_last_error(None);
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] mic capture failed to start");
                self.set_last_error(Some(e));
            }
        }
    }

    fn ensure_system_capture(&self) {
        if self.system.lock_recover().is_some() {
            return;
        }

        let session = self.session.lock_recover().clone();
        let Some(session) = session else {
            *self.system_error.lock_recover() =
                Some("meeting session is not initialized".to_string());
            return;
        };

        tracing::info!(
            session_id = %session.session_id,
            artifact_dir = %session.artifact_dir.display(),
            system_wav_path = %session.system_wav_path.display(),
            "[meeting_engine] starting system audio capture"
        );

        match start_system_capture(
            session.system_wav_path,
            Arc::clone(&self.muted),
            self.live_audio_sender(),
        ) {
            Ok(handle) => {
                *self.system.lock_recover() = Some(handle);
                *self.system_error.lock_recover() = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] system audio capture failed to start");
                *self.system_error.lock_recover() = Some(e);
            }
        }
    }

    fn stop_mic_capture(&self) -> Option<MicCaptureSummary> {
        let handle = self.mic.lock_recover().take();
        let mut handle = handle?;

        let _ = handle.stop_tx.send(());
        match handle.done_rx.recv_timeout(STOP_TIMEOUT) {
            Ok(Ok(summary)) => {
                tracing::info!(
                    path = %summary.path.display(),
                    duration_ms = summary.duration_ms,
                    samples_written = summary.samples_written,
                    dropped_chunks = summary.dropped_chunks,
                    native_rate = summary.native_rate,
                    peak = summary.peak,
                    "[meeting_engine] mic capture finalized"
                );
                *self.last_mic_summary.lock_recover() = Some(summary.clone());
                self.set_last_error(None);
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                Some(summary)
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "[meeting_engine] mic capture finalize failed");
                self.set_last_error(Some(e));
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                None
            }
            Err(e) => {
                let message = format!("timed out while stopping mic capture: {e}");
                tracing::warn!(error = %message, "[meeting_engine] mic capture stop timed out");
                self.set_last_error(Some(format!("{message}; stop is still pending")));
                *self.mic.lock_recover() = Some(handle);
                None
            }
        }
    }

    fn stop_system_capture(&self) -> Option<SystemCaptureSummary> {
        let handle = self.system.lock_recover().take();
        let mut handle = handle?;

        let _ = handle.stop_tx.send(());
        match handle.done_rx.recv_timeout(STOP_TIMEOUT) {
            Ok(Ok(summary)) => {
                tracing::info!(
                    path = %summary.path.display(),
                    duration_ms = summary.duration_ms,
                    samples_written = summary.samples_written,
                    dropped_chunks = summary.dropped_chunks,
                    native_rate = summary.native_rate,
                    peak = summary.peak,
                    "[meeting_engine] system audio capture finalized"
                );
                *self.last_system_summary.lock_recover() = Some(summary.clone());
                *self.system_error.lock_recover() = None;
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                Some(summary)
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "[meeting_engine] system audio capture finalize failed");
                *self.system_error.lock_recover() = Some(e);
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                None
            }
            Err(e) => {
                let message = format!("timed out while stopping system audio capture: {e}");
                tracing::warn!(error = %message, "[meeting_engine] system audio capture stop timed out");
                *self.system_error.lock_recover() = Some(message);
                *self.system.lock_recover() = Some(handle);
                None
            }
        }
    }

    fn prepare_transcription_source(
        &self,
        session: Option<&MeetingSession>,
        mic_summary: Option<MicCaptureSummary>,
        system_summary: Option<SystemCaptureSummary>,
    ) -> Option<MeetingTranscriptionPlan> {
        let Some(mic_summary) = mic_summary else {
            *self.audio.lock_recover() = MeetingAudioSnapshot {
                status: "skipped_missing_mic_audio".to_string(),
                error: Some("mic capture did not produce a WAV".to_string()),
                ..MeetingAudioSnapshot::default()
            };
            return None;
        };
        let mic_only_plan = |mic_summary: MicCaptureSummary| MeetingTranscriptionPlan {
            output_paths: transcript_paths_for_wav(&mic_summary.path),
            summary: mic_summary.clone(),
            source_wavs: vec![mic_summary.path.clone()],
            mic: mic_summary,
            system: None,
            source_activity_path: None,
        };

        let Some(session) = session else {
            *self.audio.lock_recover() = MeetingAudioSnapshot {
                status: "skipped_missing_session".to_string(),
                error: Some("meeting session was not available for audio merge".to_string()),
                ..MeetingAudioSnapshot::default()
            };
            return Some(mic_only_plan(mic_summary));
        };

        let meeting_mic_only_plan = |mic_summary: MicCaptureSummary| MeetingTranscriptionPlan {
            output_paths: transcript_paths_for_stem(&session.artifact_dir, "meeting"),
            summary: mic_summary.clone(),
            source_wavs: vec![mic_summary.path.clone()],
            mic: mic_summary,
            system: None,
            source_activity_path: None,
        };

        let Some(system_summary) = system_summary else {
            *self.audio.lock_recover() = MeetingAudioSnapshot {
                status: "skipped_missing_system_audio".to_string(),
                error: self
                    .system_error
                    .lock_recover()
                    .clone()
                    .or_else(|| Some("system capture did not produce a WAV".to_string())),
                ..MeetingAudioSnapshot::default()
            };
            return Some(meeting_mic_only_plan(mic_summary));
        };

        if !has_transcribable_audio(&system_summary) {
            tracing::warn!(
                peak = system_summary.peak,
                samples_written = system_summary.samples_written,
                "[meeting_engine] system audio below speech threshold; transcribing mic only"
            );
            *self.audio.lock_recover() = MeetingAudioSnapshot {
                status: "skipped_silent_system_audio".to_string(),
                error: Some("system audio was silent; transcribing mic only".to_string()),
                ..MeetingAudioSnapshot::default()
            };
            return Some(meeting_mic_only_plan(mic_summary));
        }

        match merge_meeting_audio(session, &mic_summary, &system_summary) {
            Ok(merged) => {
                let output_paths = transcript_paths_for_stem(&session.artifact_dir, "meeting");
                let source_wavs = vec![mic_summary.path.clone(), system_summary.path.clone()];
                *self.audio.lock_recover() = MeetingAudioSnapshot {
                    status: "completed".to_string(),
                    merged_path: Some(merged.summary.path.clone()),
                    source_activity_path: Some(merged.source_activity_path.clone()),
                    duration_ms: Some(merged.summary.duration_ms),
                    samples_written: merged.summary.samples_written,
                    error: None,
                };
                Some(MeetingTranscriptionPlan {
                    mic: mic_summary,
                    system: Some(system_summary),
                    summary: merged.summary,
                    output_paths,
                    source_wavs,
                    source_activity_path: Some(merged.source_activity_path),
                })
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] meeting audio merge failed");
                *self.audio.lock_recover() = MeetingAudioSnapshot {
                    status: "failed".to_string(),
                    error: Some(e),
                    ..MeetingAudioSnapshot::default()
                };
                Some(mic_only_plan(mic_summary))
            }
        }
    }

    /// Enqueue a meeting for background processing. A single worker drains the
    /// queue sequentially with retry/backoff, so ending one meeting while another
    /// is still transcribing never drops work. Duplicate enqueues for a meeting
    /// already queued or in-flight are coalesced (the regenerate/double-stop
    /// guard).
    fn start_transcription_job(&self, plan: MeetingTranscriptionPlan) {
        let meeting_id = meeting_id_from_transcript_paths(&plan.output_paths);
        self.ensure_job_worker();
        let outcome = self.jobs.enqueue(MeetingJob {
            meeting_id,
            plan: Box::new(plan),
            attempt: 0,
            not_before_ms: 0,
        });
        if outcome != EnqueueOutcome::Enqueued {
            tracing::info!(
                ?outcome,
                "[meeting_engine] transcription enqueue coalesced (already queued/running)"
            );
        }
    }

    /// Spawn the single background job worker once (idempotent).
    fn ensure_job_worker(&self) {
        {
            let mut inner = self.jobs.lock();
            if inner.worker_started {
                return;
            }
            inner.worker_started = true;
        }
        let jobs = Arc::clone(&self.jobs);
        let transcription = Arc::clone(&self.transcription);
        if let Err(e) = thread::Builder::new()
            .name("meeting-job-worker".to_string())
            .spawn(move || meeting_job_worker_loop(jobs, transcription))
        {
            tracing::error!(error = %e, "[meeting_engine] failed to spawn meeting job worker");
            self.jobs.lock().worker_started = false;
        }
    }

    /// Graceful shutdown on app exit. Stops the job worker and, if a meeting is
    /// still recording, finalizes its WAVs (valid headers + fsync + a recovery
    /// breadcrumb) so the recording survives the quit; the actual transcription
    /// is picked up next launch by `requeue_interrupted_meetings`. Bounded by the
    /// capture stop timeout so it can't hang exit.
    pub fn shutdown(&self) {
        self.jobs.request_shutdown();
        if self.active.load(Ordering::SeqCst) {
            tracing::info!("[meeting_engine] finalizing active recording on shutdown");
            let _ = self.stop();
        }
        let interrupted = self.jobs.drain_for_shutdown();
        for meeting_id in &interrupted {
            mark_meeting_interrupted_for_recovery(
                meeting_id,
                "processing interrupted because AirNote closed; it will resume on next launch",
            );
        }
        if !interrupted.is_empty() {
            tracing::warn!(
                count = interrupted.len(),
                "[meeting_engine] marked interrupted processing jobs for startup recovery"
            );
        }
        if !self.jobs.wait_until_idle(Duration::from_secs(3)) {
            tracing::warn!(
                "[meeting_engine] shutdown timed out waiting for active processing to stop; startup recovery will resume it"
            );
        }
    }

    /// Startup recovery: re-enqueue meetings that were interrupted mid-pipeline
    /// (non-terminal phase with audio or a saved transcript) so a crash or
    /// force-quit during transcription/finalization self-heals on next launch.
    /// `failed` meetings are left for the user's explicit Retry.
    pub fn requeue_interrupted_meetings(&self) {
        let root = said_core::paths::data_dir().join("meetings");
        let Ok(entries) = fs::read_dir(&root) else {
            return;
        };
        let mut requeued = 0_u32;
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let phase = read_meeting_state(&dir).map(|state| state.phase);
            if matches!(
                phase.as_deref(),
                Some(
                    MEETING_PHASE_TRANSCRIBED
                        | MEETING_PHASE_SUMMARIZED
                        | MEETING_PHASE_FAILED
                        | MEETING_PHASE_CANCELLED,
                )
            ) {
                continue;
            }
            let has_transcript = meeting_has_usable_transcript(&dir);
            // Only the tracks build_retranscribe_plan can actually use — mic or
            // the merged mixdown. A system-only or audio-less orphan dir can't be
            // re-transcribed, so skip it silently instead of log-spamming a "no
            // audio" error every launch (the storage GC reclaims those dirs).
            let has_retranscribable_audio =
                dir.join("mic.wav").is_file() || dir.join("meeting.merged.wav").is_file();
            if !has_transcript && !has_retranscribable_audio {
                continue;
            }
            match build_retranscribe_plan(&dir) {
                Ok(plan) => {
                    self.start_transcription_job(plan);
                    requeued += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, dir = %dir.display(), "[meeting_engine] could not requeue interrupted meeting");
                }
            }
        }
        if requeued > 0 {
            tracing::info!(
                count = requeued,
                "[meeting_engine] requeued interrupted meetings for transcription"
            );
        }
    }

    fn set_last_error(&self, error: Option<String>) {
        *self.last_error.lock_recover() = error;
    }

    fn emit_status(&self, app: &AppHandle) -> MeetingEngineStatus {
        let status = self.status();
        emit_main(app, STATUS_EVENT, status.clone());
        status
    }

    #[cfg(test)]
    fn install_fake_mic_capture_for_test(&self) {
        let (stop_tx, _stop_rx) = mpsc::channel();
        let (_done_tx, done_rx) = mpsc::channel();
        *self.mic.lock_recover() = Some(MicCaptureHandle {
            stop_tx,
            done_rx,
            join: None,
        });
    }

    #[cfg(test)]
    fn install_fake_system_capture_for_test(&self) {
        let (stop_tx, _stop_rx) = mpsc::channel();
        let (_done_tx, done_rx) = mpsc::channel();
        *self.system.lock_recover() = Some(SystemCaptureHandle {
            stop_tx,
            done_rx,
            join: None,
        });
    }
}

// ── Meeting job queue ────────────────────────────────────────────────────────
//
// A single background worker drains a FIFO of per-meeting jobs. This is the
// foundational fix for the "end meeting A, immediately start+end meeting B"
// case: the old single-slot guard silently dropped B. Now B is queued.
//
// Lock order (never inverted): `MeetingJobQueue.inner` is acquired and released
// quickly; `TranscriptionSnapshot` is only locked AFTER releasing `inner`. The
// worker never holds the queue lock across job execution.

const MEETING_JOB_MAX_ATTEMPTS: u32 = 3;
const MEETING_JOB_BACKOFF_BASE_MS: u64 = 2_000;
const MEETING_JOB_BACKOFF_MAX_MS: u64 = 30_000;

/// A meeting session stopped within this window of starting captured nothing
/// meaningful — typically a rapid start→stop→start (e.g. a React StrictMode
/// double-mount in dev, or a quick re-entry). Such a stop is treated as an
/// abort: the fragment is discarded without transcription, so it never marks
/// the meeting FAILED and blocks recovery of the real recording that follows.
const MEETING_MIN_SESSION_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
enum EnqueueOutcome {
    Enqueued,
    AlreadyQueued,
    AlreadyRunning,
}

struct MeetingJob {
    meeting_id: String,
    plan: Box<MeetingTranscriptionPlan>,
    attempt: u32,
    /// Earliest wall-clock ms this job may run (for retry backoff).
    not_before_ms: u64,
}

#[derive(Default)]
struct MeetingJobQueueInner {
    pending: std::collections::VecDeque<MeetingJob>,
    in_flight: Option<String>,
    cancelled: std::collections::HashSet<String>,
    worker_started: bool,
}

struct MeetingJobQueue {
    inner: Mutex<MeetingJobQueueInner>,
    cvar: Condvar,
    shutdown: AtomicBool,
}

impl MeetingJobQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MeetingJobQueueInner::default()),
            cvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Lock the queue, recovering from poisoning. A panic inside a job (caught by
    /// the worker) must never permanently brick the queue — recover the inner
    /// state rather than propagating the poison and killing every future enqueue.
    fn lock(&self) -> std::sync::MutexGuard<'_, MeetingJobQueueInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Add a job unless an equivalent one is already queued or running for the
    /// same meeting (dedup / regenerate guard).
    fn enqueue(&self, job: MeetingJob) -> EnqueueOutcome {
        let mut inner = self.lock();
        if inner.in_flight.as_deref() == Some(job.meeting_id.as_str()) {
            return EnqueueOutcome::AlreadyRunning;
        }
        if inner.pending.iter().any(|j| j.meeting_id == job.meeting_id) {
            return EnqueueOutcome::AlreadyQueued;
        }
        inner.cancelled.remove(&job.meeting_id);
        inner.pending.push_back(job);
        drop(inner);
        self.cvar.notify_one();
        EnqueueOutcome::Enqueued
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.cvar.notify_all();
    }

    fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Convert volatile in-memory queue state into durable on-disk recovery
    /// intent. Pending jobs live only in RAM, so drain them and let startup scan
    /// the meeting folders again. The in-flight job stays visible long enough
    /// for its cancel check to kill the active child process.
    fn drain_for_shutdown(&self) -> Vec<String> {
        self.request_shutdown();
        let mut inner = self.lock();
        let mut interrupted = Vec::new();
        if let Some(id) = inner.in_flight.clone() {
            interrupted.push(id);
        }
        while let Some(job) = inner.pending.pop_front() {
            interrupted.push(job.meeting_id);
        }
        interrupted.sort();
        interrupted.dedup();
        drop(inner);
        self.cvar.notify_all();
        interrupted
    }

    fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();
        loop {
            if inner.in_flight.is_none() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let remaining = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(100));
            let (guard, _) = self
                .cvar
                .wait_timeout(inner, remaining)
                .unwrap_or_else(|poison| poison.into_inner());
            inner = guard;
        }
    }

    /// True if a meeting is currently queued or being processed.
    fn is_active(&self, meeting_id: &str) -> bool {
        let inner = self.lock();
        inner.in_flight.as_deref() == Some(meeting_id)
            || inner.pending.iter().any(|j| j.meeting_id == meeting_id)
    }

    fn cancel(&self, meeting_id: &str) -> bool {
        let mut inner = self.lock();
        let before = inner.pending.len();
        inner.pending.retain(|job| job.meeting_id != meeting_id);
        let removed_pending = before != inner.pending.len();
        let in_flight = inner.in_flight.as_deref() == Some(meeting_id);
        if in_flight {
            inner.cancelled.insert(meeting_id.to_string());
        }
        drop(inner);
        self.cvar.notify_all();
        removed_pending || in_flight
    }

    fn is_cancelled(&self, meeting_id: &str) -> bool {
        self.lock().cancelled.contains(meeting_id)
    }

    fn clear_cancelled(&self, meeting_id: &str) {
        self.lock().cancelled.remove(meeting_id);
    }
}

fn meeting_id_from_transcript_paths(paths: &TranscriptPaths) -> String {
    paths
        .text
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

#[derive(Debug)]
enum JobOutcome {
    Done,
    Cancelled(String),
    Retry(String),
    Terminal(String),
}

/// Classify a transcription error as retryable (transient: process spawn,
/// timeout, IO, network) or terminal (no audio, missing binary/key — retrying
/// won't change the result).
fn classify_meeting_job_error(message: &str) -> JobOutcome {
    let m = message.to_ascii_lowercase();
    // Transient classes are checked first so a message that happens to contain
    // a terminal-ish word but is really a rate-limit / network / disk blip still
    // retries instead of permanently failing the meeting.
    if m.contains("whisper.cpp timed out") {
        return JobOutcome::Terminal(message.to_string());
    }
    let transient = m.contains("rate-limit")
        || m.contains("rate limit")
        || m.contains("429")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("network")
        || m.contains("connection")
        || m.contains("temporarily")
        || m.contains("disk")
        || m.contains("no space");
    if transient {
        return JobOutcome::Retry(message.to_string());
    }
    let terminal = m.contains("no confident speech")
        || m.contains("below speech threshold")
        || m.contains("empty")
        || m.contains("no audio")
        || m.contains("api key")
        || m.contains("_api_key")
        || m.contains("authentication failed")
        || m.contains("unauthorized")
        || m.contains("missing whisper")
        || m.contains("whisper.cpp binary")
        || m.contains("whisper.cpp crashed")
        || m.contains("binary not found")
        || m.contains("model file is missing")
        || m.contains("reinstall")
        || m.contains("no such file")
        || m.contains("no transcribable");
    if terminal {
        JobOutcome::Terminal(message.to_string())
    } else {
        JobOutcome::Retry(message.to_string())
    }
}

/// The single background worker. Pops ready jobs (respecting backoff) and runs
/// them sequentially, requeuing transient failures with exponential backoff.
fn meeting_job_worker_loop(
    jobs: Arc<MeetingJobQueue>,
    transcription: Arc<Mutex<TranscriptionSnapshot>>,
) {
    // If this thread ever exits — clean shutdown OR an escaped panic — clear the
    // started flag so the next enqueue re-spawns the worker. Without this, one
    // fatal exit would silently stop all future transcription forever.
    struct WorkerGuard(Arc<MeetingJobQueue>);
    impl Drop for WorkerGuard {
        fn drop(&mut self) {
            self.0.lock().worker_started = false;
        }
    }
    let _worker_guard = WorkerGuard(Arc::clone(&jobs));

    loop {
        let job = {
            let mut inner = jobs.lock();
            loop {
                if jobs.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                let now = now_ms();
                if let Some(pos) = inner.pending.iter().position(|j| j.not_before_ms <= now) {
                    let Some(job) = inner.pending.remove(pos) else {
                        tracing::warn!(
                            "[meeting_engine] ready job disappeared before dequeue; retrying"
                        );
                        continue;
                    };
                    inner.in_flight = Some(job.meeting_id.clone());
                    break job;
                }
                // Sleep until the soonest backoff expiry (or a long idle wait).
                let wait_ms = inner
                    .pending
                    .iter()
                    .map(|j| j.not_before_ms.saturating_sub(now))
                    .min()
                    .unwrap_or(3_600_000)
                    .clamp(50, 3_600_000);
                let (guard, _) = jobs
                    .cvar
                    .wait_timeout(inner, Duration::from_millis(wait_ms))
                    .unwrap_or_else(|poison| poison.into_inner());
                inner = guard;
            }
        };

        let is_last_attempt = job.attempt + 1 >= MEETING_JOB_MAX_ATTEMPTS;
        // Contain a job panic: a single deterministic crash must not kill the
        // worker (which would silently stop ALL future transcription). Caught
        // panics are terminal — retrying a panicking job would just loop.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_transcription_job(
                &job.meeting_id,
                &job.plan,
                &transcription,
                is_last_attempt,
                &jobs,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::error!(
                meeting_id = %job.meeting_id,
                "[meeting_engine] transcription job panicked; marking failed"
            );
            JobOutcome::Terminal("transcription job panicked".to_string())
        });

        let mut inner = jobs.lock();
        inner.in_flight = None;
        match outcome {
            JobOutcome::Retry(msg) => {
                if jobs.shutdown.load(Ordering::SeqCst) {
                    tracing::info!(
                        meeting_id = %job.meeting_id,
                        "[meeting_engine] transcription retry suppressed because app is shutting down"
                    );
                } else if inner.cancelled.remove(&job.meeting_id) {
                    tracing::info!(
                        meeting_id = %job.meeting_id,
                        "[meeting_engine] transcription retry suppressed because job was cancelled"
                    );
                } else {
                    let backoff = (MEETING_JOB_BACKOFF_BASE_MS << job.attempt)
                        .min(MEETING_JOB_BACKOFF_MAX_MS);
                    tracing::warn!(
                        meeting_id = %job.meeting_id,
                        attempt = job.attempt + 1,
                        backoff_ms = backoff,
                        error = %msg,
                        "[meeting_engine] transcription job failed; scheduling retry"
                    );
                    inner.pending.push_back(MeetingJob {
                        meeting_id: job.meeting_id,
                        plan: job.plan,
                        attempt: job.attempt + 1,
                        not_before_ms: now_ms() + backoff,
                    });
                }
            }
            JobOutcome::Cancelled(reason) => {
                inner.cancelled.remove(&job.meeting_id);
                tracing::info!(
                    meeting_id = %job.meeting_id,
                    reason = %reason,
                    "[meeting_engine] transcription job cancelled"
                );
            }
            JobOutcome::Done | JobOutcome::Terminal(_) => {
                inner.cancelled.remove(&job.meeting_id);
            }
        }
        drop(inner);
        jobs.cvar.notify_one();
    }
}

/// Execute one transcription job synchronously on the worker thread. Returns a
/// JobOutcome so the worker can decide whether to retry. On a terminal failure
/// (or the last attempt) it writes the "failed" artifact; on a retryable failure
/// with attempts remaining it leaves the on-disk phase as "transcribing" and
/// signals Retry.
fn run_transcription_job(
    meeting_id: &str,
    plan: &MeetingTranscriptionPlan,
    transcription_state: &Arc<Mutex<TranscriptionSnapshot>>,
    is_last_attempt: bool,
    jobs: &Arc<MeetingJobQueue>,
) -> JobOutcome {
    let transcript_paths = plan.output_paths.clone();
    let job_artifact_dir = transcript_paths.text.parent().map(Path::to_path_buf);
    if let Some(outcome) =
        cancelled_or_deleted_job_outcome(meeting_id, jobs, job_artifact_dir.as_deref())
    {
        return outcome;
    }
    let resume_existing_transcript = job_artifact_dir
        .as_deref()
        .is_some_and(should_resume_incomplete_transcript);
    if let Some(dir) = job_artifact_dir.as_deref() {
        write_meeting_state(dir, MEETING_PHASE_TRANSCRIBING, None);
    }
    {
        let mut transcription = transcription_state.lock_recover();
        transcription.text_path = Some(transcript_paths.text.clone());
        transcription.json_path = Some(transcript_paths.json.clone());
        transcription.language = Some(DEFAULT_WHISPER_LANGUAGE.to_string());
        transcription.provider = Some("whisper.cpp".to_string());
        transcription.model = None;
        transcription.latency_ms = None;
        transcription.text = None;
        transcription.cleaned_text = None;
        transcription.final_text = None;
        transcription.cleanup = MeetingCleanupSnapshot::idle();
        transcription.final_diarization = MeetingFinalDiarizationSnapshot::idle();
        transcription.error = None;
        transcription.running = true;
        transcription.status = "running".to_string();
        transcription.progress = None;
    }

    if resume_existing_transcript {
        if let Some(dir) = job_artifact_dir.as_deref() {
            if let Some(outcome) = cancelled_or_deleted_job_outcome(meeting_id, jobs, Some(dir)) {
                return outcome;
            }
            match resume_completed_transcript_job(plan, transcription_state, &transcript_paths, dir)
            {
                Ok(outcome) => return outcome,
                Err(e) => {
                    tracing::warn!(error = %e, dir = %dir.display(), "[meeting_engine] saved transcript resume failed; re-running transcription");
                }
            }
        }
    }

    // Empty audio → terminal skip (re-running won't help).
    if plan.mic.samples_written == 0
        && plan
            .system
            .as_ref()
            .map(|summary| summary.samples_written == 0)
            .unwrap_or(true)
    {
        let message = "meeting WAV tracks are empty; skipping transcription".to_string();
        let cleanup = MeetingCleanupSnapshot::skipped("skipped_no_audio", message.clone());
        {
            let mut transcription = transcription_state.lock_recover();
            transcription.running = false;
            transcription.status = "skipped_empty_audio".to_string();
            transcription.progress = None;
            transcription.cleanup = cleanup.clone();
            transcription.final_diarization = MeetingFinalDiarizationSnapshot::skipped(
                "skipped_no_transcript",
                message.clone(),
                final_diarization_paths_for_transcript(&transcript_paths),
            );
            transcription.error = Some(message.clone());
        }
        write_transcript_artifact(
            &transcript_paths,
            &plan.summary,
            "skipped_empty_audio",
            None,
            DEFAULT_WHISPER_LANGUAGE,
            "",
            None,
            None,
            cleanup,
            Vec::new(),
            plan.source_wavs.clone(),
            Some(message.clone()),
        );
        if let Some(dir) = job_artifact_dir.as_deref() {
            write_meeting_state(dir, MEETING_PHASE_FAILED, Some(message.clone()));
        }
        return JobOutcome::Terminal(message);
    }

    let config = match resolve_whisper_cpp_config() {
        Ok(config) => config,
        Err(e) => {
            let cleanup = MeetingCleanupSnapshot::skipped("skipped_missing_whisper", e.clone());
            {
                let mut transcription = transcription_state.lock_recover();
                transcription.running = false;
                transcription.status = "skipped_missing_whisper".to_string();
                transcription.progress = None;
                transcription.cleanup = cleanup.clone();
                transcription.final_diarization = MeetingFinalDiarizationSnapshot::skipped(
                    "skipped_no_transcript",
                    e.clone(),
                    final_diarization_paths_for_transcript(&transcript_paths),
                );
                transcription.error = Some(e.clone());
            }
            write_transcript_artifact(
                &transcript_paths,
                &plan.summary,
                "skipped_missing_whisper",
                None,
                DEFAULT_WHISPER_LANGUAGE,
                "",
                None,
                None,
                cleanup,
                Vec::new(),
                plan.source_wavs.clone(),
                Some(e.clone()),
            );
            return JobOutcome::Terminal(e);
        }
    };

    {
        let mut transcription = transcription_state.lock_recover();
        transcription.language = Some(config.language.clone());
        transcription.model = Some(config.model.to_string_lossy().to_string());
    }

    if let Some(outcome) =
        cancelled_or_deleted_job_outcome(meeting_id, jobs, job_artifact_dir.as_deref())
    {
        return outcome;
    }

    let cancel_requested = || {
        jobs.is_shutting_down()
            || jobs.is_cancelled(meeting_id)
            || job_artifact_dir.as_deref().is_some_and(|dir| !dir.exists())
    };
    let report_progress = |progress: MeetingProcessingProgress| {
        let mut transcription = transcription_state.lock_recover();
        if transcription.running && transcription.status == "running" {
            transcription.progress = Some(progress);
        }
    };

    match transcribe_meeting_plan(
        plan,
        &config,
        Some(&cancel_requested),
        Some(&report_progress),
    ) {
        Ok(mut done) => {
            if let Some(outcome) =
                cancelled_or_deleted_job_outcome(meeting_id, jobs, job_artifact_dir.as_deref())
            {
                return outcome;
            }
            let cleanup_config = meeting_cleanup_config();
            let cleanup_provider = cleanup_config
                .as_ref()
                .map(|config| config.provider.clone())
                .unwrap_or_else(|_| meeting_cleanup_provider());
            let cleanup_model = cleanup_config
                .as_ref()
                .map(|config| config.model.clone())
                .unwrap_or_else(|_| meeting_cleanup_model(&cleanup_provider));
            {
                let mut transcription = transcription_state.lock_recover();
                transcription.status = "cleaning".to_string();
                transcription.progress = None;
                transcription.latency_ms = Some(done.latency_ms);
                transcription.text = Some(done.transcript.clone());
                transcription.cleaned_text = None;
                transcription.final_text = None;
                transcription.cleanup = MeetingCleanupSnapshot::running(
                    cleanup_provider.clone(),
                    cleanup_model.clone(),
                );
                transcription.error = None;
            }

            let cleanup_started = Instant::now();
            let cleanup_result = cleanup_config
                .and_then(|config| cleanup_meeting_transcript_with_llm(&done.transcript, config));
            let (mut cleaned_transcript, cleanup) = match cleanup_result {
                Ok(result) => (
                    Some(result.transcript.clone()),
                    MeetingCleanupSnapshot::completed(&result),
                ),
                Err(e) if e.contains("API key") || e.contains("_API_KEY") => (
                    None,
                    MeetingCleanupSnapshot::skipped("skipped_missing_key", e),
                ),
                Err(e) => (
                    None,
                    MeetingCleanupSnapshot::failed(
                        cleanup_provider,
                        cleanup_model,
                        cleanup_started.elapsed().as_millis() as u64,
                        e,
                    ),
                ),
            };

            // After-meeting: convert the Devanagari transcript → Roman Hinglish
            // using the Groq cleanup pipeline (per-segment, alignment preserved),
            // falling back to the deterministic romanizer if the LLM doesn't line
            // up. Live captions stayed in native script; deepseek does the summary.
            if config.romanize && said_core::script::contains_devanagari(&done.transcript) {
                let groq = meeting_cleanup_config()
                    .and_then(|cfg| transliterate_segments_with_llm(&mut done.segments, cfg));
                if let Err(e) = groq {
                    tracing::warn!(error = %e, "[meeting_engine] Groq transliteration failed; using deterministic romanizer");
                    for segment in &mut done.segments {
                        segment.text = said_core::script::romanize_devanagari(&segment.text);
                    }
                } else {
                    tracing::info!(
                        "[meeting_engine] transliterated transcript to Hinglish via Groq"
                    );
                }
                done.transcript = format_meeting_timeline_transcript(&done.segments);
            }

            match name_meeting_speakers_with_ai(&mut done.segments, cleaned_transcript.as_deref()) {
                Ok(replacements) => {
                    if !replacements.is_empty() {
                        if let Some(text) = cleaned_transcript.as_mut() {
                            *text = rewrite_speaker_labels_in_text(text, &replacements);
                        }
                        done.transcript = format_meeting_timeline_transcript(&done.segments);
                        tracing::info!(
                            count = replacements.len(),
                            "[meeting_engine] inferred meeting speaker names from transcript context"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[meeting_engine] speaker naming skipped; keeping generic labels"
                    );
                }
            }

            if let Some(outcome) =
                cancelled_or_deleted_job_outcome(meeting_id, jobs, job_artifact_dir.as_deref())
            {
                return outcome;
            }

            write_transcript_artifact(
                &transcript_paths,
                &done.summary,
                "completed",
                Some(&config),
                &config.language,
                &done.transcript,
                Some(done.latency_ms),
                cleaned_transcript.as_deref(),
                cleanup.clone(),
                done.segments.clone(),
                done.source_wavs.clone(),
                None,
            );

            finish_transcribed_meeting(
                transcription_state,
                &transcript_paths,
                &done.summary.path,
                job_artifact_dir.as_deref(),
                done.transcript,
                cleaned_transcript,
                cleanup,
                Some(done.latency_ms),
            )
        }
        Err(e) => {
            if is_cancelled_subprocess_error(&e) {
                if let Some(outcome) =
                    cancelled_or_deleted_job_outcome(meeting_id, jobs, job_artifact_dir.as_deref())
                {
                    return outcome;
                }
                return JobOutcome::Cancelled(e);
            }
            tracing::warn!(error = %e, "[meeting_engine] whisper.cpp transcription failed");
            let terminal = matches!(classify_meeting_job_error(&e), JobOutcome::Terminal(_))
                || is_last_attempt;
            if !terminal {
                // Leave the on-disk phase as "transcribing"; the worker will retry.
                return JobOutcome::Retry(e);
            }
            if let Some(dir) = job_artifact_dir.as_deref() {
                write_meeting_state(dir, MEETING_PHASE_FAILED, Some(e.clone()));
            }
            write_transcript_artifact(
                &transcript_paths,
                &plan.summary,
                "failed",
                Some(&config),
                &config.language,
                "",
                None,
                None,
                MeetingCleanupSnapshot::skipped("skipped_no_transcript", e.clone()),
                Vec::new(),
                plan.source_wavs.clone(),
                Some(e.clone()),
            );
            {
                let mut transcription = transcription_state.lock_recover();
                transcription.running = false;
                transcription.status = "failed".to_string();
                transcription.progress = None;
                transcription.cleanup =
                    MeetingCleanupSnapshot::skipped("skipped_no_transcript", e.clone());
                transcription.final_diarization = MeetingFinalDiarizationSnapshot::skipped(
                    "skipped_no_transcript",
                    e.clone(),
                    final_diarization_paths_for_transcript(&transcript_paths),
                );
                transcription.error = Some(e.clone());
            }
            JobOutcome::Terminal(e)
        }
    }
}

fn cancelled_or_deleted_job_outcome(
    meeting_id: &str,
    jobs: &MeetingJobQueue,
    artifact_dir: Option<&Path>,
) -> Option<JobOutcome> {
    if jobs.is_cancelled(meeting_id) {
        if let Some(dir) = artifact_dir.filter(|dir| dir.is_dir()) {
            write_meeting_state(
                dir,
                MEETING_PHASE_CANCELLED,
                Some("processing cancelled by user".to_string()),
            );
        }
        return Some(JobOutcome::Cancelled(
            "processing cancelled by user".to_string(),
        ));
    }
    if artifact_dir.is_some_and(|dir| !dir.exists()) {
        return Some(JobOutcome::Cancelled(
            "meeting files were deleted during processing".to_string(),
        ));
    }
    None
}

fn mark_meeting_interrupted_for_recovery(meeting_id: &str, reason: &str) {
    let Ok(dir) = meeting_dir_for_id(meeting_id) else {
        return;
    };
    if !dir.is_dir() {
        return;
    }
    if read_meeting_state(&dir).is_some_and(|state| {
        matches!(
            state.phase.as_str(),
            MEETING_PHASE_TRANSCRIBED
                | MEETING_PHASE_SUMMARIZED
                | MEETING_PHASE_FAILED
                | MEETING_PHASE_CANCELLED
        )
    }) {
        return;
    }
    write_meeting_state(&dir, MEETING_PHASE_TRANSCRIBING, Some(reason.to_string()));
}

fn should_resume_incomplete_transcript(dir: &Path) -> bool {
    let Some(state) = read_meeting_state(dir) else {
        return false;
    };
    if matches!(
        state.phase.as_str(),
        MEETING_PHASE_TRANSCRIBED
            | MEETING_PHASE_SUMMARIZED
            | MEETING_PHASE_FAILED
            | MEETING_PHASE_CANCELLED
    ) {
        return false;
    }
    meeting_has_usable_transcript(dir)
}

fn resume_completed_transcript_job(
    plan: &MeetingTranscriptionPlan,
    transcription_state: &Arc<Mutex<TranscriptionSnapshot>>,
    transcript_paths: &TranscriptPaths,
    artifact_dir: &Path,
) -> Result<JobOutcome, String> {
    let artifact = read_meeting_transcript_artifact(&transcript_paths.json)?;
    if artifact.status != "completed" || artifact.transcript.trim().is_empty() {
        return Err("saved transcript artifact is not completed".to_string());
    }

    let cleanup = MeetingCleanupSnapshot {
        status: artifact.cleanup_status.clone(),
        provider: artifact.cleanup_provider.clone(),
        model: artifact.cleanup_model.clone(),
        latency_ms: artifact.cleanup_latency_ms,
        error: artifact.cleanup_error.clone(),
    };

    {
        let mut transcription = transcription_state
            .lock()
            .expect("meeting engine lock poisoned");
        transcription.language = artifact.language.clone();
        transcription.provider = Some(artifact.provider.clone());
        transcription.model = artifact.model.clone();
        transcription.latency_ms = artifact.latency_ms;
        transcription.text = Some(artifact.transcript.clone());
        transcription.cleaned_text = artifact.cleaned_transcript.clone();
        transcription.final_text = None;
        transcription.cleanup = cleanup.clone();
        transcription.final_diarization = MeetingFinalDiarizationSnapshot::idle();
        transcription.error = None;
        transcription.running = true;
        transcription.status = "resuming".to_string();
        transcription.progress = None;
    }

    tracing::info!(
        dir = %artifact_dir.display(),
        "[meeting_engine] resuming meeting pipeline from saved transcript artifact"
    );

    Ok(finish_transcribed_meeting(
        transcription_state,
        transcript_paths,
        &plan.summary.path,
        Some(artifact_dir),
        artifact.transcript,
        artifact.cleaned_transcript,
        cleanup,
        artifact.latency_ms,
    ))
}

fn finish_transcribed_meeting(
    transcription_state: &Arc<Mutex<TranscriptionSnapshot>>,
    _transcript_paths: &TranscriptPaths,
    _audio_path: &Path,
    artifact_dir: Option<&Path>,
    raw_transcript: String,
    cleaned_transcript: Option<String>,
    cleanup: MeetingCleanupSnapshot,
    latency_ms: Option<u64>,
) -> JobOutcome {
    let final_diarization = MeetingFinalDiarizationSnapshot::idle();
    let final_transcript_text = None;

    // Capture the best transcript text for the summary stage before the
    // originals are moved into the snapshot below.
    let (summary_source, summary_text) = if let Some(text) = &cleaned_transcript {
        ("cleaned".to_string(), text.clone())
    } else {
        ("raw".to_string(), raw_transcript.clone())
    };

    {
        let mut transcription = transcription_state
            .lock()
            .expect("meeting engine lock poisoned");
        transcription.running = false;
        transcription.status = "completed".to_string();
        transcription.progress = None;
        transcription.latency_ms = latency_ms;
        transcription.text = Some(raw_transcript);
        transcription.cleaned_text = cleaned_transcript;
        transcription.final_text = final_transcript_text;
        transcription.cleanup = cleanup;
        transcription.final_diarization = final_diarization;
        transcription.error = None;
    }

    // Checkpoint: transcript + finalization are on disk.
    if let Some(dir) = artifact_dir {
        if dir.join("meeting.ai.json").is_file() {
            write_meeting_state(dir, MEETING_PHASE_SUMMARIZED, None);
            prune_meeting_intermediates(dir);
            return JobOutcome::Done;
        }

        write_meeting_state(dir, MEETING_PHASE_TRANSCRIBED, None);
        // The transcript is durable now — reclaim the disposable intermediates
        // (live/ windows + *.asr.wav copies/chunks), the bulk of a meeting's
        // footprint.
        prune_meeting_intermediates(dir);

        // Final stage: generate the meeting summary so the after-meeting flow is
        // complete and robust — NOT a separate manual click.
        // run_meeting_intelligence's LLM call already retries transient
        // failures; on a terminal failure we record it in meeting state (phase
        // stays "transcribed" + a "summary failed" error) so the UI surfaces it
        // and offers Retry, instead of silently stopping. On success
        // write_meeting_intelligence_cache marks the meeting "summarized".
        if env_bool("AIRNOTE_MEETING_AUTO_SUMMARY", true) && !summary_text.trim().is_empty() {
            if let Ok(mut t) = transcription_state.lock() {
                t.status = "summarizing".to_string();
                t.progress = None;
            }
            let selected = MeetingAiTranscript {
                source: summary_source,
                text: summary_text,
            };
            match run_meeting_intelligence(selected, Some(dir.to_path_buf())) {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, dir = %dir.display(), "[meeting_engine] meeting summary generation failed");
                    write_meeting_state(
                        dir,
                        MEETING_PHASE_TRANSCRIBED,
                        Some(format!("summary failed: {e}")),
                    );
                }
            }
            if let Ok(mut t) = transcription_state.lock() {
                t.status = "completed".to_string();
                t.progress = None;
            }
        }
    }

    JobOutcome::Done
}

#[tauri::command]
pub fn meeting_engine_start_session(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
    meeting_id: Option<String>,
) -> MeetingEngineStatus {
    tracing::info!(meeting_id = ?meeting_id, "[meeting_engine] start session");
    let status = state.start(meeting_id, Some(app.clone()));
    emit_main(&app, STATUS_EVENT, status.clone());
    status
}

/// The session id doubles as the on-disk artifact directory name, so a
/// caller-supplied meeting id is only accepted when it is a safe path
/// component (ASCII alphanumerics, `-`, `_`; bounded length). Anything else —
/// including ids with separators or traversal sequences — is rejected so the
/// caller falls back to a generated `local-<ts>-<gen>` id.
fn safe_meeting_dir_id(meeting_id: Option<&str>) -> Option<String> {
    let id = meeting_id?.trim();
    if id.is_empty() || id.len() > 128 {
        return None;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .then(|| id.to_string())
}

// MUST be async. In Tauri v2 a synchronous command runs on the MAIN thread; in
// optimized/release builds the meeting's recurring IPC saturates the main thread
// tightly enough that a sync `stop_session` never gets dispatched — "End meeting"
// hangs and this fn is never even entered (its first log line never appears).
// (Making the pollers async alone was insufficient for release builds.) As an
// async command it runs on the async runtime, so End always dispatches. The body
// has no `.await`, so `State<'_>` is fine; `stop()` is bounded by STOP_TIMEOUT.
#[tauri::command]
pub async fn meeting_engine_stop_session(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> Result<MeetingEngineStatus, String> {
    tracing::info!("[meeting_engine] stop session (invoke)");
    let status = state.stop();
    emit_main(&app, STATUS_EVENT, status.clone());
    Ok(status)
}

/// Stop the active meeting and broadcast the new status. Idempotent (no-ops when
/// nothing is recording). This is the backend-authoritative End path: it is
/// driven by the `meeting/request-stop` EVENT (see main.rs setup), NOT an invoke,
/// so on Windows it can never be orphaned by the LiveMeetingView unmounting in the
/// same tick the End is fired (a Tauri `invoke` callback is torn down with the
/// view, leaving the meeting recording forever — "Couldn't find callback id").
/// An event has no per-call JS callback, so delivery is independent of the view.
pub fn request_stop(app: &AppHandle, state: &MeetingEngineState) -> MeetingEngineStatus {
    tracing::info!("[meeting_engine] request_stop (event)");
    let status = state.stop();
    emit_main(app, STATUS_EVENT, status.clone());
    status
}

#[tauri::command]
pub fn meeting_engine_toggle_mute(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> MeetingEngineStatus {
    tracing::info!("[meeting_engine] toggle mute");
    let status = state.toggle_mute();
    emit_main(&app, STATUS_EVENT, status.clone());
    status
}

// NOTE: `async` is load-bearing, not cosmetic. In Tauri v2 a *synchronous*
// command runs on the main thread; an `async` one runs on the async runtime.
// The frontend polls `get_status` every 1s, and `status()` locks ~10 mutexes —
// as a sync command that recurring poll occupies the main thread and can starve
// dispatch of the (sync) `meeting_engine_stop_session`, so "End meeting" hangs
// (the command never even logs). Making the read-only pollers async keeps the
// main thread free to dispatch control commands. Read-only → no ordering risk.
#[tauri::command]
pub async fn meeting_engine_get_status(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> Result<MeetingEngineStatus, String> {
    Ok(state.emit_status(&app))
}

#[tauri::command]
pub async fn meeting_engine_get_live_transcript(
    state: State<'_, MeetingEngineState>,
) -> Result<MeetingLiveTranscriptPayload, String> {
    Ok(state.live_transcript_payload())
}

#[tauri::command]
pub async fn meeting_engine_generate_intelligence(
    state: State<'_, MeetingEngineState>,
    meeting_id: Option<String>,
) -> Result<MeetingIntelligenceResult, String> {
    // Regenerate guard: if this meeting is still being transcribed/cleaned, the
    // transcript isn't final yet — don't kick off a summary against partial text.
    if let Some(id) = meeting_id.as_deref() {
        if state.jobs.is_active(id) {
            return Err("This meeting is still being processed; the summary will be available once transcription finishes.".to_string());
        }
    }
    // Resolve transcript + target dir (touch state/disk), then run the blocking
    // LLM off the main thread so the UI never freezes during analysis.
    let selected = resolve_intelligence_transcript(&state, meeting_id.as_deref())?;
    let target_dir = intelligence_target_dir(&state, meeting_id.as_deref());
    tauri::async_runtime::spawn_blocking(move || run_meeting_intelligence(selected, target_dir))
        .await
        .map_err(|e| format!("meeting intelligence task failed: {e}"))?
}

// Async (off the main thread) for the same reason as get_processing_status —
// these read per-meeting artifacts and must not block IPC dispatch when opened
// against a still-recording meeting.
#[tauri::command]
pub fn meeting_engine_get_cached_intelligence(
    state: State<'_, MeetingEngineState>,
    meeting_id: Option<String>,
) -> Result<Option<MeetingIntelligenceResult>, String> {
    load_cached_meeting_intelligence(&state, meeting_id.as_deref())
}

#[tauri::command]
pub fn meeting_engine_get_cached_artifacts(
    state: State<'_, MeetingEngineState>,
    meeting_id: Option<String>,
) -> Result<Option<MeetingCachedArtifacts>, String> {
    load_cached_meeting_artifacts(&state, meeting_id.as_deref())
}

/// Lightweight per-meeting summary used to populate the meeting list sidebar
/// without loading each meeting's full artifacts. Built strictly from a
/// meeting's own folder.
#[derive(Debug, Default, Serialize)]
pub struct MeetingOverview {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    tags: Vec<String>,
    action_count: usize,
    decision_count: usize,
    word_count: usize,
    has_intelligence: bool,
    favorite: bool,
    hidden: bool,
    /// Real recording files (audio or transcript) are still on disk — i.e. the
    /// meeting was removed from the list but not file-deleted. Drives "Archived".
    has_local_files: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lark_doc_url: Option<String>,
}

/// Local, per-meeting user overrides that can't live on the server (the
/// control-plane meeting record is read-only from here): a renamed title,
/// favorite flag, hidden flag, and dismissed AI tags. Stored in one registry
/// file under `meetings/` so it survives even when a meeting's artifact folder
/// is deleted, and so the whole list can be read in one shot.
const MEETING_OVERRIDES_FILE: &str = ".user-overrides.json";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct MeetingOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    dismissed_tags: Vec<String>,
    // URL of the Lark doc this meeting was exported to (idempotency + "Open in
    // Lark"). Survives re-analysis and file deletion since it lives in the
    // top-level registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lark_doc_url: Option<String>,
}

impl MeetingOverride {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && !self.favorite
            && !self.hidden
            && self.dismissed_tags.is_empty()
            && self.lark_doc_url.is_none()
    }
}

type MeetingOverrides = std::collections::HashMap<String, MeetingOverride>;

fn meeting_overrides_path() -> PathBuf {
    said_core::paths::data_dir()
        .join("meetings")
        .join(MEETING_OVERRIDES_FILE)
}

fn read_meeting_overrides() -> MeetingOverrides {
    let path = meeting_overrides_path();
    let Ok(bytes) = fs::read(&path) else {
        // Missing file → genuinely empty; safe to start fresh.
        return MeetingOverrides::default();
    };
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return MeetingOverrides::default();
    }
    match serde_json::from_slice(&bytes) {
        Ok(map) => map,
        Err(e) => {
            // The file exists and is non-empty but won't parse. Returning a
            // default here would let the next write clobber every meeting's
            // title/favorite/hidden/lark-url. Preserve the bytes for recovery
            // and log loudly instead of silently wiping the whole history.
            let backup = path.with_extension(format!("corrupt-{}", now_ms()));
            if let Err(rename_err) = fs::rename(&path, &backup) {
                tracing::error!(error = %rename_err, "[meeting_engine] failed to back up corrupt overrides file");
            } else {
                tracing::error!(error = %e, backup = %backup.display(), "[meeting_engine] meeting overrides file was corrupt; backed up and reset");
            }
            MeetingOverrides::default()
        }
    }
}

fn write_meeting_overrides(map: &MeetingOverrides) -> Result<(), String> {
    let path = meeting_overrides_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create meetings dir: {e}"))?;
    }
    let bytes = serde_json::to_vec_pretty(map)
        .map_err(|e| format!("failed to serialize overrides: {e}"))?;
    write_atomic(path, bytes).map_err(|e| format!("failed to write overrides: {e}"))
}

fn update_meeting_override<F: FnOnce(&mut MeetingOverride)>(
    meeting_id: &str,
    mutate: F,
) -> Result<(), String> {
    let id =
        safe_meeting_dir_id(Some(meeting_id)).ok_or_else(|| "invalid meeting id".to_string())?;
    let mut map = read_meeting_overrides();
    let entry = map.entry(id.clone()).or_default();
    mutate(entry);
    if entry.is_empty() {
        map.remove(&id);
    }
    write_meeting_overrides(&map)
}

/// Count words in a meeting's transcript without a full parse — reads the first
/// available transcript text file and splits on whitespace.
fn meeting_dir_word_count(dir: &Path) -> usize {
    for name in [
        "meeting.transcript.final.txt",
        "meeting.transcript.txt",
        "mic.transcript.txt",
    ] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            let count = text.split_whitespace().count();
            if count > 0 {
                return count;
            }
        }
    }
    0
}

#[tauri::command]
pub fn meeting_engine_get_meeting_overviews(
    meeting_ids: Vec<String>,
) -> std::collections::HashMap<String, MeetingOverview> {
    let overrides = read_meeting_overrides();
    let mut overviews = std::collections::HashMap::new();
    for id in meeting_ids {
        let ov = overrides.get(&id).cloned().unwrap_or_default();
        let mut overview = MeetingOverview {
            favorite: ov.favorite,
            hidden: ov.hidden,
            lark_doc_url: ov.lark_doc_url.clone(),
            ..MeetingOverview::default()
        };
        // Read cached intelligence/word count only if the folder still exists
        // (it may have been file-deleted while the server record lingers).
        if let Ok(dir) = meeting_dir_for_id(&id) {
            if dir.is_dir() {
                overview.has_local_files = RECOVERABLE_MEETING_WAVS
                    .iter()
                    .any(|name| dir.join(name).is_file())
                    || meeting_has_usable_transcript(&dir);
                if let Ok(Some(intel)) = load_cached_meeting_intelligence_from_dir(&dir) {
                    overview.has_intelligence = true;
                    let title = intel.title.trim();
                    if !title.is_empty() {
                        overview.title = Some(title.to_string());
                    }
                    overview.tags = intel.tags;
                    overview.action_count = intel.action_items.len();
                    overview.decision_count = intel.decisions.len();
                }
                overview.word_count = meeting_dir_word_count(&dir);
            }
        }
        // A user-renamed title wins over the AI title.
        if ov.title.is_some() {
            overview.title = ov.title;
        }
        // Drop dismissed AI tags.
        if !ov.dismissed_tags.is_empty() {
            overview.tags.retain(|tag| {
                !ov.dismissed_tags
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(tag))
            });
        }
        overviews.insert(id, overview);
    }
    overviews
}

// ===================== Local-only meeting list (no cloud) =====================
//
// Meetings are a fully local, single-device feature: the list is the set of
// meeting folders on disk, not a control-plane query. This is what eliminates
// empty "Quick meeting" rows at the root — there is no eager cloud record and no
// org-shared backlog to inherit.

/// One meeting in the local list. Mirrors the fields the meetings UI reads from
/// the old cloud `Meeting` + the per-meeting `MeetingOverview`, so the frontend
/// can build both from a single call.
#[derive(Clone, Debug, Serialize)]
pub struct LocalMeetingSummary {
    id: String,
    title: String,
    /// "live" while it is the active recording session, otherwise "ended".
    status: String,
    /// Creation time (ms since epoch): parsed from a `local-<ms>-<gen>` id, or
    /// the folder mtime for legacy cloud-id folders.
    created_at_ms: u64,
    tags: Vec<String>,
    action_count: usize,
    decision_count: usize,
    word_count: usize,
    has_intelligence: bool,
    favorite: bool,
    hidden: bool,
    has_local_files: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lark_doc_url: Option<String>,
}

/// Parse the creation time from a `local-<ms>-<gen>` id; None for other ids.
fn created_at_from_local_id(id: &str) -> Option<u64> {
    id.strip_prefix("local-")?.split('-').next()?.parse().ok()
}

/// Folder mtime in ms — fallback creation time for legacy cloud-id folders.
fn dir_mtime_ms(dir: &Path) -> u64 {
    fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Allocate a fresh local meeting id. New meetings are local-only — no cloud
/// record is created, so abandoned ones never leave an empty "Quick meeting".
#[tauri::command]
pub fn meeting_engine_new_local_meeting() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    format!("local-{}-{}", now_ms(), seq)
}

/// List every meeting stored locally on this device — the source of truth for
/// the meetings list (replaces the cloud `GET /v1/meetings`).
///
/// Async (off the main thread): this iterates EVERY meeting dir and parses each
/// (word counts, intelligence, overrides), so it can take meaningful time once
/// many meetings accumulate — and it gets slower right after a meeting ends
/// while the final transcription job is writing. As a sync command it ran on the
/// main thread, blocking all ipc://localhost dispatch for its full duration;
/// combined with the list page's poll loop that exhausted the ~6-connection pool
/// and wedged the page on "Loading meetings…" forever. Running on the tokio
/// runtime keeps the main thread free to service other invokes.
#[tauri::command]
pub fn meeting_engine_list_meetings(
    state: State<'_, MeetingEngineState>,
) -> Vec<LocalMeetingSummary> {
    let active_id = state
        .session
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|session| session.session_id.clone()));
    let overrides = read_meeting_overrides();
    let root = said_core::paths::data_dir().join("meetings");
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut out: Vec<LocalMeetingSummary> = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        // Skip the overrides/digests dotfolders and anything not a valid id.
        if id.starts_with('.') || safe_meeting_dir_id(Some(&id)).is_none() {
            continue;
        }

        let is_active = active_id.as_deref() == Some(id.as_str());
        let has_local_files = RECOVERABLE_MEETING_WAVS
            .iter()
            .any(|name| dir.join(name).is_file())
            || meeting_has_usable_transcript(&dir);
        let intel = load_cached_meeting_intelligence_from_dir(&dir)
            .ok()
            .flatten();
        let has_intelligence = intel.is_some();
        // Drop genuinely empty/abandoned folders (no audio, no transcript, no
        // summary, not currently recording) — they never enter the list.
        if !has_local_files && !has_intelligence && !is_active {
            continue;
        }

        let ov = overrides.get(&id).cloned().unwrap_or_default();
        let ai_title = intel
            .as_ref()
            .map(|i| i.title.trim().to_string())
            .filter(|t| !t.is_empty());
        let title = ov
            .title
            .clone()
            .or(ai_title)
            .unwrap_or_else(|| "Untitled meeting".to_string());
        let mut tags = intel.as_ref().map(|i| i.tags.clone()).unwrap_or_default();
        if !ov.dismissed_tags.is_empty() {
            tags.retain(|tag| {
                !ov.dismissed_tags
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(tag))
            });
        }

        out.push(LocalMeetingSummary {
            id: id.clone(),
            title,
            status: if is_active { "live" } else { "ended" }.to_string(),
            created_at_ms: created_at_from_local_id(&id).unwrap_or_else(|| dir_mtime_ms(&dir)),
            tags,
            action_count: intel.as_ref().map(|i| i.action_items.len()).unwrap_or(0),
            decision_count: intel.as_ref().map(|i| i.decisions.len()).unwrap_or(0),
            word_count: meeting_dir_word_count(&dir),
            has_intelligence,
            favorite: ov.favorite,
            hidden: ov.hidden,
            has_local_files,
            lark_doc_url: ov.lark_doc_url.clone(),
        });
    }

    out.sort_by_key(|m| std::cmp::Reverse(m.created_at_ms));
    out
}

/// Max transcript words for a meeting to count as "empty" — silence/noise that
/// whisper turned into a word or two. Above this it has real content.
const EMPTY_MEETING_MAX_WORDS: usize = 5;

/// True if the meeting folder still holds a track with real, transcribable
/// speech energy (not just silence/noise). Repairs WAV size headers first so an
/// interrupted, never-finalized recording is measured correctly instead of
/// reading as empty. Used to keep "clear empty meetings" from deleting a
/// recording whose transcript merely failed or hasn't run yet.
fn meeting_has_recoverable_speech(dir: &Path) -> bool {
    for name in RECOVERABLE_MEETING_WAVS {
        let wav = dir.join(name);
        if !wav.is_file() {
            continue;
        }
        let _ = repair_wav_header_sizes(&wav);
        if let Some(summary) = capture_summary_from_wav(&wav) {
            if has_transcribable_audio(&summary) {
                return true;
            }
        }
    }
    false
}

/// A locally-stored meeting with effectively no content: never analyzed, not
/// favorited, no user-set title, and only a few transcript words.
fn meeting_is_empty(dir: &Path, id: &str, overrides: &MeetingOverrides) -> bool {
    if let Some(ov) = overrides.get(id) {
        if ov.favorite || ov.title.is_some() {
            return false;
        }
    }
    if dir.join("meeting.ai.json").is_file() {
        return false;
    }
    if meeting_dir_word_count(dir) >= EMPTY_MEETING_MAX_WORDS {
        return false;
    }
    // Few/no transcript words — but if the recording still contains real speech,
    // it's a recording whose transcription failed or hasn't run yet, NOT an
    // empty silence/noise clip. Preserve it so a bulk "clear empty" never
    // deletes recoverable audio. Genuine silence has no transcribable energy and
    // is still cleared.
    !meeting_has_recoverable_speech(dir)
}

/// Delete every empty meeting on this device (silence/noise recordings never
/// acted on). Returns how many were removed. The active recording, and any
/// analyzed / favorited / renamed meeting, are always kept.
#[tauri::command]
pub fn meeting_engine_clear_empty_meetings(state: State<'_, MeetingEngineState>) -> usize {
    let active_id = state
        .session
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|session| session.session_id.clone()));
    let overrides = read_meeting_overrides();
    let root = said_core::paths::data_dir().join("meetings");
    let Ok(entries) = fs::read_dir(&root) else {
        return 0;
    };
    let mut cleared = 0usize;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if id.starts_with('.') || safe_meeting_dir_id(Some(&id)).is_none() {
            continue;
        }
        // Never touch the meeting that's currently recording.
        if active_id.as_deref() == Some(id.as_str()) {
            continue;
        }
        if meeting_is_empty(&dir, &id, &overrides) && fs::remove_dir_all(&dir).is_ok() {
            cleared += 1;
        }
    }
    cleared
}

#[cfg(test)]
mod local_list_tests {
    use super::*;

    #[test]
    fn created_at_parses_from_local_id_only() {
        assert_eq!(
            created_at_from_local_id("local-1718500000123-0"),
            Some(1_718_500_000_123)
        );
        assert_eq!(created_at_from_local_id("local-42-7"), Some(42));
        assert_eq!(
            created_at_from_local_id("00460b07-8e36-4b0e-9985-e6219955bbb5"),
            None
        );
        assert_eq!(created_at_from_local_id("local-notanumber-1"), None);
        assert_eq!(created_at_from_local_id("random"), None);
    }

    #[test]
    fn new_local_meeting_ids_are_unique_and_parseable() {
        let a = meeting_engine_new_local_meeting();
        let b = meeting_engine_new_local_meeting();
        assert_ne!(a, b);
        assert!(a.starts_with("local-"));
        assert!(created_at_from_local_id(&a).is_some());
    }
}

#[tauri::command]
pub fn meeting_engine_set_meeting_title(
    meeting_id: String,
    title: Option<String>,
) -> Result<(), String> {
    // Empty / whitespace clears the override and reverts to the AI/server title.
    let title = title
        .and_then(nonempty_trimmed)
        .map(|t| t.chars().take(120).collect::<String>());
    update_meeting_override(&meeting_id, |o| o.title = title)
}

#[tauri::command]
pub fn meeting_engine_set_meeting_favorite(
    meeting_id: String,
    favorite: bool,
) -> Result<(), String> {
    update_meeting_override(&meeting_id, |o| o.favorite = favorite)
}

#[tauri::command]
pub fn meeting_engine_set_meeting_hidden(meeting_id: String, hidden: bool) -> Result<(), String> {
    update_meeting_override(&meeting_id, |o| o.hidden = hidden)
}

#[tauri::command]
pub fn meeting_engine_set_meeting_lark_doc(
    meeting_id: String,
    url: Option<String>,
) -> Result<(), String> {
    let url = url.and_then(nonempty_trimmed);
    update_meeting_override(&meeting_id, |o| o.lark_doc_url = url)
}

#[tauri::command]
pub fn meeting_engine_dismiss_ai_tag(meeting_id: String, tag: String) -> Result<(), String> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Ok(());
    }
    update_meeting_override(&meeting_id, |o| {
        if !o
            .dismissed_tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&tag))
        {
            o.dismissed_tags.push(tag);
        }
    })
}

/// Permanently delete a meeting's local artifact folder (audio, transcript,
/// summary, tags). Keeps it hidden in the registry so it stays out of the list.
/// The server record is untouched (no server delete endpoint).
#[tauri::command]
pub fn meeting_engine_delete_meeting_files(
    state: State<'_, MeetingEngineState>,
    meeting_id: String,
) -> Result<(), String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    state.jobs.cancel(&meeting_id);
    if dir.is_dir() {
        fs::remove_dir_all(&dir).map_err(|e| format!("failed to delete meeting files: {e}"))?;
    }
    update_meeting_override(&meeting_id, |o| o.hidden = true)
}

#[tauri::command]
pub fn meeting_engine_cancel_processing(
    state: State<'_, MeetingEngineState>,
    meeting_id: String,
) -> Result<(), String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let cancelled = state.jobs.cancel(&meeting_id);
    if dir.is_dir() {
        write_meeting_state(
            &dir,
            MEETING_PHASE_CANCELLED,
            Some("processing cancelled by user".to_string()),
        );
    }
    if cancelled || dir.is_dir() {
        Ok(())
    } else {
        Err("This meeting is not being processed.".to_string())
    }
}

/// User-added tags for a meeting, stored locally beside its artifacts and kept
/// separate from AI-generated tags so re-analysing the meeting never erases the
/// user's own tags.
const MEETING_USER_TAGS_FILE: &str = "meeting.tags.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct MeetingUserTags {
    #[serde(default)]
    tags: Vec<String>,
}

fn meeting_dir_for_id(meeting_id: &str) -> Result<PathBuf, String> {
    let id =
        safe_meeting_dir_id(Some(meeting_id)).ok_or_else(|| "invalid meeting id".to_string())?;
    Ok(said_core::paths::data_dir().join("meetings").join(id))
}

fn read_meeting_user_tags(dir: &Path) -> Vec<String> {
    fs::read(dir.join(MEETING_USER_TAGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<MeetingUserTags>(&bytes).ok())
        .map(|stored| stored.tags)
        .unwrap_or_default()
}

fn write_meeting_user_tags(dir: &Path, tags: &[String]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&MeetingUserTags {
        tags: tags.to_vec(),
    })
    .map_err(|e| format!("failed to serialize meeting tags: {e}"))?;
    write_atomic(dir.join(MEETING_USER_TAGS_FILE), bytes)
        .map_err(|e| format!("failed to write meeting tags: {e}"))
}

fn sanitize_user_tag(tag: &str) -> Result<String, String> {
    let tag = tag.trim().trim_start_matches('#').trim();
    if tag.is_empty() {
        return Err("tag is empty".to_string());
    }
    if tag.chars().count() > 32 {
        return Err("tag must be 32 characters or fewer".to_string());
    }
    Ok(tag.to_string())
}

#[tauri::command]
pub fn meeting_engine_get_user_tags(meeting_id: String) -> Result<Vec<String>, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    Ok(read_meeting_user_tags(&dir))
}

#[tauri::command]
pub fn meeting_engine_add_user_tag(meeting_id: String, tag: String) -> Result<Vec<String>, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let tag = sanitize_user_tag(&tag)?;
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create meeting directory: {e}"))?;
    let mut tags = read_meeting_user_tags(&dir);
    if !tags
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&tag))
    {
        tags.push(tag);
        write_meeting_user_tags(&dir, &tags)?;
    }
    Ok(tags)
}

#[tauri::command]
pub fn meeting_engine_remove_user_tag(
    meeting_id: String,
    tag: String,
) -> Result<Vec<String>, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let target = tag.trim();
    let mut tags = read_meeting_user_tags(&dir);
    let before = tags.len();
    tags.retain(|existing| !existing.eq_ignore_ascii_case(target));
    if tags.len() != before {
        write_meeting_user_tags(&dir, &tags)?;
    }
    Ok(tags)
}

/// Free-form personal notes a user writes for a meeting, stored locally beside
/// its artifacts. Kept separate from the AI summary and used as extra context
/// for the meeting chat.
const MEETING_NOTES_FILE: &str = "meeting.notes.md";

#[tauri::command]
pub fn meeting_engine_get_notes(meeting_id: String) -> Result<String, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    Ok(fs::read_to_string(dir.join(MEETING_NOTES_FILE)).unwrap_or_default())
}

#[tauri::command]
pub fn meeting_engine_set_notes(meeting_id: String, notes: String) -> Result<(), String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let path = dir.join(MEETING_NOTES_FILE);
    if notes.trim().is_empty() {
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        return Ok(());
    }
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create meeting directory: {e}"))?;
    write_atomic(path, notes).map_err(|e| format!("failed to write notes: {e}"))
}

/// User-added action items for a meeting (separate from AI-detected ones), kept
/// locally beside the artifacts and used as extra chat/export context.
const MEETING_MANUAL_ACTIONS_FILE: &str = "meeting.manual-actions.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManualAction {
    title: String,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ManualActions {
    #[serde(default)]
    items: Vec<ManualAction>,
}

#[tauri::command]
pub fn meeting_engine_get_manual_actions(meeting_id: String) -> Result<Vec<ManualAction>, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let items = fs::read(dir.join(MEETING_MANUAL_ACTIONS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ManualActions>(&bytes).ok())
        .map(|stored| stored.items)
        .unwrap_or_default();
    Ok(items)
}

#[tauri::command]
pub fn meeting_engine_set_manual_actions(
    meeting_id: String,
    items: Vec<ManualAction>,
) -> Result<(), String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    // Drop blank titles; trim.
    let items: Vec<ManualAction> = items
        .into_iter()
        .filter_map(|a| {
            let title = a.title.trim().to_string();
            if title.is_empty() {
                None
            } else {
                Some(ManualAction {
                    title,
                    done: a.done,
                })
            }
        })
        .collect();
    let path = dir.join(MEETING_MANUAL_ACTIONS_FILE);
    if items.is_empty() {
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        return Ok(());
    }
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create meeting directory: {e}"))?;
    let bytes = serde_json::to_vec_pretty(&ManualActions { items })
        .map_err(|e| format!("failed to serialize manual actions: {e}"))?;
    write_atomic(path, bytes).map_err(|e| format!("failed to write manual actions: {e}"))
}

/// Re-run transcription (whisper → cleanup → summary) on a meeting's saved
/// audio using the current language/model settings. Salvages recordings whose
/// transcript was produced with the wrong language. Runs as the standard
/// background transcription job; the frontend polls `meeting_engine_get_status`
/// until it finishes, then reloads the artifacts.
#[tauri::command]
pub fn meeting_engine_retranscribe(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
    meeting_id: String,
) -> Result<MeetingEngineStatus, String> {
    if state.active.load(Ordering::SeqCst) {
        return Err("Stop the live meeting before re-transcribing.".to_string());
    }
    let dir = meeting_dir_for_id(&meeting_id)?;
    if !dir.is_dir() {
        return Err("This meeting has no local audio to re-transcribe.".to_string());
    }
    if state.jobs.is_active(&meeting_id) {
        return Err("This meeting is already being processed; please wait.".to_string());
    }
    let plan = build_retranscribe_plan(&dir)?;
    state.start_transcription_job(plan);
    let status = state.status();
    emit_main(&app, STATUS_EVENT, status.clone());
    Ok(status)
}

// ── Full-text meeting search ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MeetingSearchInput {
    id: String,
    #[serde(default)]
    title: String,
}

#[derive(Debug, Serialize)]
pub struct MeetingSearchHit {
    id: String,
    score: i32,
    matched_in: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

/// Read the first available transcript text for search indexing.
fn meeting_search_transcript_text(dir: &Path) -> String {
    for name in [
        "meeting.transcript.final.txt",
        "meeting.transcript.txt",
        "mic.transcript.txt",
    ] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// Build a short snippet centered on the first matched term.
fn meeting_search_snippet(text: &str, terms: &[String]) -> Option<String> {
    let lower = text.to_lowercase();
    let pos = terms.iter().filter_map(|t| lower.find(t.as_str())).min()?;
    let start = text[..pos]
        .char_indices()
        .rev()
        .nth(48)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = text[pos..]
        .char_indices()
        .nth(120)
        .map(|(i, _)| pos + i)
        .unwrap_or(text.len());
    let mut snippet = text[start..end]
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < text.len() {
        snippet.push('…');
    }
    Some(snippet)
}

/// Search meetings across title, tags, summary, decisions, action items, notes,
/// and transcript. The caller passes ids + server titles (the backend doesn't
/// have those); everything else is read from each meeting's local files. A
/// meeting matches only when EVERY whitespace-separated query term appears in
/// at least one field (AND); hits are scored by field weight, best-first, with
/// a snippet.
#[tauri::command]
pub fn meeting_engine_search_meetings(
    query: String,
    meetings: Vec<MeetingSearchInput>,
) -> Vec<MeetingSearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let terms: Vec<String> = query.split_whitespace().map(str::to_string).collect();
    let overrides = read_meeting_overrides();
    let mut hits: Vec<MeetingSearchHit> = Vec::new();

    for meeting in meetings {
        let ov = overrides.get(&meeting.id).cloned().unwrap_or_default();
        let mut fields: Vec<(&'static str, i32, String)> = Vec::new();

        let title = ov
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(meeting.title);
        fields.push(("title", 6, title));

        let mut tag_text = String::new();
        let mut summary = String::new();
        let mut decisions = String::new();
        let mut actions = String::new();
        let mut notes = String::new();
        let mut transcript = String::new();

        if let Ok(dir) = meeting_dir_for_id(&meeting.id) {
            if dir.is_dir() {
                if let Ok(Some(intel)) = load_cached_meeting_intelligence_from_dir(&dir) {
                    let kept: Vec<String> = intel
                        .tags
                        .into_iter()
                        .filter(|t| !ov.dismissed_tags.iter().any(|d| d.eq_ignore_ascii_case(t)))
                        .collect();
                    tag_text = kept.join(" ");
                    summary = intel.summary;
                    decisions = intel
                        .decisions
                        .iter()
                        .map(|d| format!("{} {}", d.text, d.evidence.as_deref().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join("  ");
                    actions = intel
                        .action_items
                        .iter()
                        .map(|a| {
                            format!(
                                "{} {} {}",
                                a.title,
                                a.assignee.as_deref().unwrap_or(""),
                                a.evidence.as_deref().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("  ");
                }
                let user_tags = read_meeting_user_tags(&dir);
                if !user_tags.is_empty() {
                    if !tag_text.is_empty() {
                        tag_text.push(' ');
                    }
                    tag_text.push_str(&user_tags.join(" "));
                }
                notes = fs::read_to_string(dir.join(MEETING_NOTES_FILE)).unwrap_or_default();
                transcript = meeting_search_transcript_text(&dir);
            }
        }

        fields.push(("tags", 5, tag_text));
        fields.push(("decisions", 4, decisions));
        fields.push(("actions", 4, actions));
        fields.push(("notes", 4, notes));
        fields.push(("summary", 3, summary));
        fields.push(("transcript", 1, transcript));

        let combined = fields
            .iter()
            .map(|(_, _, text)| text.to_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        if !terms.iter().all(|term| combined.contains(term.as_str())) {
            continue;
        }

        let mut matched_in: Vec<String> = Vec::new();
        let mut score = 0;
        let mut snippet: Option<String> = None;
        for (name, weight, text) in &fields {
            let lower = text.to_lowercase();
            let hit_count = terms.iter().filter(|t| lower.contains(t.as_str())).count();
            if hit_count > 0 {
                matched_in.push((*name).to_string());
                score += weight * hit_count as i32;
                if snippet.is_none() && !matches!(*name, "title" | "tags") {
                    snippet = meeting_search_snippet(text, &terms);
                }
            }
        }

        hits.push(MeetingSearchHit {
            id: meeting.id,
            score,
            matched_in,
            snippet,
        });
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits
}

/// Tauri event carrying one streamed chunk of a chat answer. The frontend
/// filters by `request_id` and appends `delta` to the in-progress bubble.
#[derive(Clone, Serialize)]
struct MeetingChatDelta {
    request_id: String,
    delta: String,
}

const MEETING_CHAT_DELTA_EVENT: &str = "meeting-chat-delta";

#[tauri::command]
pub async fn meeting_engine_chat(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
    request_id: String,
    question: String,
    summary: Option<String>,
    transcript_override: Option<String>,
    notes: Option<String>,
) -> Result<MeetingChatResult, String> {
    // Resolve the transcript (this reads engine state) up front, then run the
    // blocking LLM request on a blocking thread. Declaring the command `async`
    // keeps it off the main thread, so the webview never freezes while the LLM
    // responds. Each streamed chunk is emitted as a `meeting-chat-delta` event
    // so the answer renders token-by-token; the final result is returned so the
    // frontend can replace the streamed text with the cleaned answer + metadata.
    let selected = resolve_meeting_chat_transcript(&state, transcript_override.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        answer_meeting_question(
            selected,
            &question,
            summary.as_deref(),
            notes.as_deref(),
            |delta| {
                // Token-by-token from a spawn_blocking thread — marshal onto the
                // main thread so the cross-thread emit can't contend with the
                // WebView2 IPC on Windows (see emit_main).
                emit_main(
                    &app,
                    MEETING_CHAT_DELTA_EVENT,
                    MeetingChatDelta {
                        request_id: request_id.clone(),
                        delta: delta.to_string(),
                    },
                );
            },
        )
    })
    .await
    .map_err(|e| format!("meeting chat task failed: {e}"))?
}

fn start_mic_capture(
    path: PathBuf,
    muted: Arc<AtomicBool>,
    live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
) -> Result<MicCaptureHandle, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<Result<MicCaptureSummary, String>>();
    let capture_path = path.clone();

    let join = thread::Builder::new()
        .name("meeting-mic-capture".to_string())
        .spawn(move || {
            let result = run_mic_capture(capture_path, muted, live_audio_tx, stop_rx, ready_tx);
            let _ = done_tx.send(result);
        })
        .map_err(|e| format!("failed to spawn mic capture thread: {e}"))?;

    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => Ok(MicCaptureHandle {
            stop_tx,
            done_rx,
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(e) => {
            drop(stop_tx);
            Err(format!("mic capture did not become ready: {e}"))
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_system_capture(
    path: PathBuf,
    muted: Arc<AtomicBool>,
    live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
) -> Result<SystemCaptureHandle, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<Result<SystemCaptureSummary, String>>();
    let capture_path = path.clone();

    let join = thread::Builder::new()
        .name("meeting-system-audio-capture".to_string())
        .spawn(move || {
            let result =
                run_system_audio_capture(capture_path, muted, live_audio_tx, stop_rx, ready_tx);
            let _ = done_tx.send(result);
        })
        .map_err(|e| format!("failed to spawn system audio capture thread: {e}"))?;

    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => Ok(SystemCaptureHandle {
            stop_tx,
            done_rx,
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(e) => {
            drop(stop_tx);
            Err(format!("system audio capture did not become ready: {e}"))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn start_system_capture(
    _path: PathBuf,
    _muted: Arc<AtomicBool>,
    _live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
) -> Result<SystemCaptureHandle, String> {
    Err("system audio capture is only available on macOS and Windows in this phase".to_string())
}

fn run_mic_capture(
    path: PathBuf,
    muted: Arc<AtomicBool>,
    live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<(), String>>,
) -> Result<MicCaptureSummary, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            report_ready_error(
                &ready_tx,
                format!("failed to create mic artifact directory: {e}"),
            )
        })?;
    }

    let host = cpal::default_host();
    let device =
        said_recorder::select_input_device(&host).map_err(|e| report_ready_error(&ready_tx, e))?;
    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
    let default_config = device
        .default_input_config()
        .map_err(|e| report_ready_error(&ready_tx, format!("no default input config: {e}")))?;

    let native_rate = default_config.sample_rate().0;
    let native_channels = default_config.channels();
    let sample_format = default_config.sample_format();
    let config = default_config.config();
    tracing::info!(
        input_device = %device_name,
        native_rate,
        native_channels,
        ?sample_format,
        "[meeting_engine] opened mic input"
    );
    let (audio_tx, audio_rx) = mpsc::sync_channel::<Vec<i16>>(AUDIO_QUEUE_DEPTH);
    let dropped_chunks = Arc::new(AtomicU64::new(0));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let writer =
        create_audio_wav_writer(&path, "mic").map_err(|e| report_ready_error(&ready_tx, e))?;

    let err_cb = |err: cpal::StreamError| {
        tracing::warn!(error = %err, "[meeting_engine] mic stream error");
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let audio_tx = audio_tx.clone();
            let dropped_chunks = Arc::clone(&dropped_chunks);
            let muted = Arc::clone(&muted);
            let live_audio_tx = live_audio_tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = mono_from_f32(data, native_channels);
                    enqueue_resampled_pcm(
                        mono,
                        native_rate,
                        &muted,
                        &audio_tx,
                        &dropped_chunks,
                        live_audio_tx.as_ref(),
                        LiveAudioSource::Mic,
                    );
                },
                err_cb,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let audio_tx = audio_tx.clone();
            let dropped_chunks = Arc::clone(&dropped_chunks);
            let muted = Arc::clone(&muted);
            let live_audio_tx = live_audio_tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mono = mono_from_i16(data, native_channels);
                    enqueue_resampled_pcm(
                        mono,
                        native_rate,
                        &muted,
                        &audio_tx,
                        &dropped_chunks,
                        live_audio_tx.as_ref(),
                        LiveAudioSource::Mic,
                    );
                },
                err_cb,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let audio_tx = audio_tx.clone();
            let dropped_chunks = Arc::clone(&dropped_chunks);
            let muted = Arc::clone(&muted);
            let live_audio_tx = live_audio_tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let mono = mono_from_u16(data, native_channels);
                    enqueue_resampled_pcm(
                        mono,
                        native_rate,
                        &muted,
                        &audio_tx,
                        &dropped_chunks,
                        live_audio_tx.as_ref(),
                        LiveAudioSource::Mic,
                    );
                },
                err_cb,
                None,
            )
        }
        _other => Err(cpal::BuildStreamError::StreamConfigNotSupported),
    }
    .map_err(|e| {
        report_ready_error(
            &ready_tx,
            format!("failed to build mic input stream ({sample_format:?}): {e}"),
        )
    })?;

    let writer_path = path.clone();
    let writer_dropped_chunks = Arc::clone(&dropped_chunks);
    let writer_stop_flag = Arc::clone(&writer_stop);
    let writer_join = thread::Builder::new()
        .name("meeting-mic-writer".to_string())
        .spawn(move || {
            write_audio_wav(
                &writer_path,
                writer,
                audio_rx,
                native_rate,
                writer_dropped_chunks,
                "mic",
                writer_stop_flag,
            )
        })
        .map_err(|e| {
            report_ready_error(&ready_tx, format!("failed to spawn mic writer thread: {e}"))
        })?;

    if let Err(e) = stream.play() {
        drop(audio_tx);
        let _ = writer_join.join();
        let message = format!("failed to start mic input stream: {e}");
        let _ = ready_tx.send(Err(message.clone()));
        return Err(message);
    }

    let _ = ready_tx.send(Ok(()));
    let _ = stop_rx.recv();
    writer_stop.store(true, Ordering::SeqCst);
    drop(stream);
    drop(audio_tx);

    writer_join
        .join()
        .map_err(|_| "mic writer thread panicked".to_string())?
}

#[cfg(target_os = "macos")]
fn run_system_audio_capture(
    path: PathBuf,
    muted: Arc<AtomicBool>,
    live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<(), String>>,
) -> Result<SystemCaptureSummary, String> {
    use screencapturekit::prelude::*;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            report_ready_error(
                &ready_tx,
                format!("failed to create system audio artifact directory: {e}"),
            )
        })?;
    }

    let content = SCShareableContent::get().map_err(|e| {
        report_ready_error(
            &ready_tx,
            format!("ScreenCaptureKit shareable content failed: {e}"),
        )
    })?;
    let display = content.displays().into_iter().next().ok_or_else(|| {
        report_ready_error(
            &ready_tx,
            "ScreenCaptureKit returned no display".to_string(),
        )
    })?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build();
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(SAMPLE_RATE as i32)
        .with_channel_count(CHANNELS as i32)
        .with_excludes_current_process_audio(true);

    let (audio_tx, audio_rx) = mpsc::sync_channel::<Vec<i16>>(AUDIO_QUEUE_DEPTH);
    let dropped_chunks = Arc::new(AtomicU64::new(0));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let writer =
        create_audio_wav_writer(&path, "system").map_err(|e| report_ready_error(&ready_tx, e))?;
    let writer_path = path.clone();
    let writer_dropped_chunks = Arc::clone(&dropped_chunks);
    let writer_stop_flag = Arc::clone(&writer_stop);
    let writer_join = thread::Builder::new()
        .name("meeting-system-audio-writer".to_string())
        .spawn(move || {
            write_audio_wav(
                &writer_path,
                writer,
                audio_rx,
                SAMPLE_RATE,
                writer_dropped_chunks,
                "system",
                writer_stop_flag,
            )
        })
        .map_err(|e| {
            report_ready_error(
                &ready_tx,
                format!("failed to spawn system audio writer thread: {e}"),
            )
        })?;

    let mut stream = SCStream::new(&filter, &config);
    let handler_tx = audio_tx.clone();
    let handler_muted = Arc::clone(&muted);
    let handler_dropped_chunks = Arc::clone(&dropped_chunks);
    let handler_live_audio_tx = live_audio_tx.clone();
    let handler_id = stream.add_output_handler(
        move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            if handler_muted.load(Ordering::SeqCst) {
                return;
            }
            if let Some(samples) = system_samples_from_buffer(&sample) {
                let pcm: Vec<i16> = samples.into_iter().map(float_to_i16).collect();
                enqueue_pcm(
                    pcm,
                    &handler_tx,
                    &handler_dropped_chunks,
                    handler_live_audio_tx.as_ref(),
                    LiveAudioSource::System,
                );
            }
        },
        SCStreamOutputType::Audio,
    );
    if handler_id.is_none() {
        drop(stream);
        drop(audio_tx);
        let _ = writer_join.join();
        let message = "ScreenCaptureKit failed to add system audio output handler".to_string();
        let _ = ready_tx.send(Err(message.clone()));
        return Err(message);
    }

    if let Err(e) = stream.start_capture() {
        drop(stream);
        drop(audio_tx);
        let _ = writer_join.join();
        let message = format!("ScreenCaptureKit system audio capture failed: {e}");
        let _ = ready_tx.send(Err(message.clone()));
        return Err(message);
    }

    let _ = ready_tx.send(Ok(()));
    let _ = stop_rx.recv();
    writer_stop.store(true, Ordering::SeqCst);
    let _ = stream.stop_capture();
    drop(stream);
    drop(audio_tx);

    writer_join
        .join()
        .map_err(|_| "system audio writer thread panicked".to_string())?
}

#[cfg(target_os = "windows")]
fn run_system_audio_capture(
    path: PathBuf,
    muted: Arc<AtomicBool>,
    live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<(), String>>,
) -> Result<SystemCaptureSummary, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            report_ready_error(
                &ready_tx,
                format!("failed to create system audio artifact directory: {e}"),
            )
        })?;
    }

    let _com = initialize_wasapi_com()
        .map_err(|e| report_ready_error(&ready_tx, format!("WASAPI COM init failed: {e}")))?;
    let (audio_client, capture_client, mix_format) = open_wasapi_loopback_capture()
        .map_err(|e| report_ready_error(&ready_tx, format!("WASAPI loopback open failed: {e}")))?;

    tracing::info!(
        native_rate = mix_format.sample_rate,
        native_channels = mix_format.channels,
        block_align = mix_format.block_align,
        sample_format = ?mix_format.sample_format,
        "[meeting_engine] opened WASAPI system loopback"
    );

    let (audio_tx, audio_rx) = mpsc::sync_channel::<Vec<i16>>(AUDIO_QUEUE_DEPTH);
    let dropped_chunks = Arc::new(AtomicU64::new(0));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let writer =
        create_audio_wav_writer(&path, "system").map_err(|e| report_ready_error(&ready_tx, e))?;
    let writer_path = path.clone();
    let writer_dropped_chunks = Arc::clone(&dropped_chunks);
    let writer_stop_flag = Arc::clone(&writer_stop);
    let writer_join = thread::Builder::new()
        .name("meeting-system-audio-writer".to_string())
        .spawn(move || {
            write_audio_wav(
                &writer_path,
                writer,
                audio_rx,
                SAMPLE_RATE,
                writer_dropped_chunks,
                "system",
                writer_stop_flag,
            )
        })
        .map_err(|e| {
            report_ready_error(
                &ready_tx,
                format!("failed to spawn system audio writer thread: {e}"),
            )
        })?;

    if let Err(e) = unsafe { audio_client.Start() } {
        writer_stop.store(true, Ordering::SeqCst);
        drop(audio_tx);
        let _ = writer_join.join();
        let message = format!("failed to start WASAPI loopback stream: {e}");
        let _ = ready_tx.send(Err(message.clone()));
        return Err(message);
    }

    let _ = ready_tx.send(Ok(()));
    let poll_interval = Duration::from_millis(10);
    let mut capture_error: Option<String> = None;
    loop {
        match stop_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if let Err(e) = unsafe {
            drain_wasapi_loopback_packets(
                &capture_client,
                &mix_format,
                &muted,
                &audio_tx,
                &dropped_chunks,
                live_audio_tx.as_ref(),
            )
        } {
            capture_error = Some(e);
            break;
        }
        thread::sleep(poll_interval);
    }

    let _ = unsafe { audio_client.Stop() };
    writer_stop.store(true, Ordering::SeqCst);
    drop(audio_tx);

    let summary = writer_join
        .join()
        .map_err(|_| "system audio writer thread panicked".to_string())?;

    if let Some(error) = capture_error {
        Err(error)
    } else {
        summary
    }
}

#[cfg(target_os = "windows")]
struct WasapiComGuard {
    uninitialize: bool,
}

#[cfg(target_os = "windows")]
impl Drop for WasapiComGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn initialize_wasapi_com() -> Result<WasapiComGuard, String> {
    use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(format!("{hr:?}"));
    }
    Ok(WasapiComGuard {
        uninitialize: hr == S_OK || hr == S_FALSE,
    })
}

#[cfg(target_os = "windows")]
fn open_wasapi_loopback_capture() -> Result<
    (
        windows::Win32::Media::Audio::IAudioClient,
        windows::Win32::Media::Audio::IAudioCaptureClient,
        WasapiMixFormat,
    ),
    String,
> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient,
        IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator) failed: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("GetDefaultAudioEndpoint(render/console) failed: {e}"))?;
        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate(IAudioClient) failed: {e}"))?;
        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat failed: {e}"))?;
        if mix_format_ptr.is_null() {
            return Err("GetMixFormat returned null".to_string());
        }

        let mix_format = match wasapi_mix_format_from_ptr(mix_format_ptr) {
            Ok(format) => format,
            Err(e) => {
                CoTaskMemFree(Some(mix_format_ptr as *const std::ffi::c_void));
                return Err(e);
            }
        };

        let init_result = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            mix_format_ptr,
            None,
        );
        CoTaskMemFree(Some(mix_format_ptr as *const std::ffi::c_void));
        init_result.map_err(|e| format!("IAudioClient::Initialize(loopback) failed: {e}"))?;

        let capture_client: IAudioCaptureClient = audio_client
            .GetService()
            .map_err(|e| format!("GetService(IAudioCaptureClient) failed: {e}"))?;
        Ok((audio_client, capture_client, mix_format))
    }
}

#[cfg(target_os = "windows")]
unsafe fn wasapi_mix_format_from_ptr(
    format_ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> Result<WasapiMixFormat, String> {
    use windows::Win32::Media::Audio::{WAVE_FORMAT_PCM, WAVEFORMATEXTENSIBLE};
    use windows::Win32::Media::KernelStreaming::{
        KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE,
    };
    use windows::Win32::Media::Multimedia::{
        KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT,
    };

    let format = unsafe { *format_ptr };
    let channels = format.nChannels;
    let sample_rate = format.nSamplesPerSec;
    let block_align = format.nBlockAlign;
    let bits_per_sample = format.wBitsPerSample;
    let format_tag = format.wFormatTag;
    if channels == 0 || sample_rate == 0 || block_align == 0 {
        return Err(format!(
            "invalid WASAPI mix format: channels={channels}, sample_rate={sample_rate}, block_align={block_align}"
        ));
    }

    let sample_format = if format_tag == WAVE_FORMAT_IEEE_FLOAT as u16 {
        WindowsLoopbackSampleFormat::F32
    } else if format_tag == WAVE_FORMAT_PCM as u16 {
        pcm_sample_format_for_bits(bits_per_sample)?
    } else if format_tag == WAVE_FORMAT_EXTENSIBLE as u16
        && format.cbSize as usize
            >= std::mem::size_of::<WAVEFORMATEXTENSIBLE>().saturating_sub(std::mem::size_of::<
                windows::Win32::Media::Audio::WAVEFORMATEX,
            >())
    {
        let extensible = unsafe { *(format_ptr as *const WAVEFORMATEXTENSIBLE) };
        let subformat = unsafe { std::ptr::addr_of!(extensible.SubFormat).read_unaligned() };
        if subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
            WindowsLoopbackSampleFormat::F32
        } else if subformat == KSDATAFORMAT_SUBTYPE_PCM {
            let valid_bits = unsafe { extensible.Samples.wValidBitsPerSample };
            pcm_sample_format_for_bits(if valid_bits == 0 {
                bits_per_sample
            } else {
                valid_bits
            })?
        } else {
            return Err(format!(
                "unsupported WASAPI extensible subformat: {:?}",
                subformat
            ));
        }
    } else {
        return Err(format!(
            "unsupported WASAPI mix format tag={format_tag}, bits_per_sample={bits_per_sample}"
        ));
    };

    let bytes_per_sample = bytes_per_windows_loopback_sample(channels, block_align, sample_format)
        .ok_or_else(|| {
            format!(
                "invalid WASAPI sample container: channels={channels}, block_align={block_align}, sample_format={sample_format:?}"
            )
        })? as u16;

    Ok(WasapiMixFormat {
        channels,
        sample_rate,
        block_align,
        bytes_per_sample,
        sample_format,
    })
}

#[cfg(target_os = "windows")]
unsafe fn drain_wasapi_loopback_packets(
    capture_client: &windows::Win32::Media::Audio::IAudioCaptureClient,
    mix_format: &WasapiMixFormat,
    muted: &AtomicBool,
    audio_tx: &mpsc::SyncSender<Vec<i16>>,
    dropped_chunks: &AtomicU64,
    live_audio_tx: Option<&mpsc::SyncSender<LiveAudioChunk>>,
) -> Result<(), String> {
    use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;

    let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }
        .map_err(|e| format!("WASAPI GetNextPacketSize failed: {e}"))?;
    while packet_frames > 0 {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;
        unsafe { capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
            .map_err(|e| format!("WASAPI GetBuffer failed: {e}"))?;

        if frames > 0 {
            let mono = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                vec![0.0; frames as usize]
            } else {
                let byte_len = frames as usize * mix_format.block_align as usize;
                let bytes = unsafe { std::slice::from_raw_parts(data.cast_const(), byte_len) };
                decode_windows_loopback_frames_to_mono(
                    bytes,
                    frames,
                    mix_format.channels,
                    mix_format.block_align,
                    mix_format.bytes_per_sample,
                    mix_format.sample_format,
                )
            };
            enqueue_resampled_pcm(
                mono,
                mix_format.sample_rate,
                muted,
                audio_tx,
                dropped_chunks,
                live_audio_tx,
                LiveAudioSource::System,
            );
        }

        unsafe { capture_client.ReleaseBuffer(frames) }
            .map_err(|e| format!("WASAPI ReleaseBuffer failed: {e}"))?;
        packet_frames = unsafe { capture_client.GetNextPacketSize() }
            .map_err(|e| format!("WASAPI GetNextPacketSize failed: {e}"))?;
    }
    Ok(())
}

fn report_ready_error(ready_tx: &mpsc::Sender<Result<(), String>>, message: String) -> String {
    let _ = ready_tx.send(Err(message.clone()));
    message
}

fn create_audio_wav_writer(
    path: &Path,
    source_label: &str,
) -> Result<hound::WavWriter<BufWriter<File>>, String> {
    let file =
        File::create(path).map_err(|e| format!("failed to create {source_label} WAV: {e}"))?;
    let spec = hound::WavSpec {
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::new(BufWriter::new(file), spec)
        .map_err(|e| format!("failed to initialize {source_label} WAV writer: {e}"))
}

fn write_audio_wav(
    path: &Path,
    mut writer: hound::WavWriter<BufWriter<File>>,
    audio_rx: mpsc::Receiver<Vec<i16>>,
    native_rate: u32,
    dropped_chunks: Arc<AtomicU64>,
    source_label: &str,
    stop_writing: Arc<AtomicBool>,
) -> Result<MicCaptureSummary, String> {
    let mut samples_written = 0_u64;
    let mut peak_i16 = 0_i16;

    loop {
        if stop_writing.load(Ordering::SeqCst) {
            drain_audio_wav_queue(
                &mut writer,
                &audio_rx,
                source_label,
                &mut samples_written,
                &mut peak_i16,
            )?;
            break;
        }

        let chunk = match audio_rx.recv_timeout(CAPTURE_WRITER_STOP_POLL) {
            Ok(chunk) => Some(chunk),
            Err(mpsc::RecvTimeoutError::Timeout) if stop_writing.load(Ordering::SeqCst) => {
                drain_audio_wav_queue(
                    &mut writer,
                    &audio_rx,
                    source_label,
                    &mut samples_written,
                    &mut peak_i16,
                )?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let Some(chunk) = chunk else {
            continue;
        };
        write_audio_chunk_to_wav(
            &mut writer,
            chunk,
            source_label,
            &mut samples_written,
            &mut peak_i16,
        )?;
    }

    writer
        .finalize()
        .map_err(|e| format!("failed to finalize {source_label} WAV: {e}"))?;
    repair_wav_header_sizes(path)?;
    // Irreplaceable source audio — force it to disk so a power loss after this
    // returns can't leave a truncated/zero-length recording.
    fsync_file(path);
    let duration_ms = samples_written.saturating_mul(1_000) / SAMPLE_RATE as u64;
    let peak = peak_i16 as f32 / i16::MAX as f32;

    Ok(MicCaptureSummary {
        path: path.to_path_buf(),
        samples_written,
        dropped_chunks: dropped_chunks.load(Ordering::SeqCst),
        native_rate,
        duration_ms,
        peak,
    })
}

fn drain_audio_wav_queue(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    audio_rx: &mpsc::Receiver<Vec<i16>>,
    source_label: &str,
    samples_written: &mut u64,
    peak_i16: &mut i16,
) -> Result<(), String> {
    while let Ok(chunk) = audio_rx.try_recv() {
        write_audio_chunk_to_wav(writer, chunk, source_label, samples_written, peak_i16)?;
    }
    Ok(())
}

fn write_audio_chunk_to_wav(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    chunk: Vec<i16>,
    source_label: &str,
    samples_written: &mut u64,
    peak_i16: &mut i16,
) -> Result<(), String> {
    for sample in chunk {
        writer
            .write_sample(sample)
            .map_err(|e| format!("failed to write {source_label} sample: {e}"))?;
        *samples_written += 1;
        *peak_i16 = (*peak_i16).max(sample.saturating_abs());
    }
    Ok(())
}

fn start_live_transcript_worker(
    session: MeetingSession,
    config: LiveTranscriptConfig,
    snapshot: Arc<Mutex<LiveTranscriptSnapshot>>,
    app: Option<AppHandle>,
) -> Result<LiveTranscriptHandle, String> {
    let live_dir = session.artifact_dir.join("live");
    fs::create_dir_all(&live_dir)
        .map_err(|e| format!("failed to create live transcript directory: {e}"))?;

    {
        let mut live = snapshot.lock_recover();
        live.session_id = Some(session.session_id.clone());
        live.running = true;
        live.status = "running".to_string();
        live.provider = Some("whisper.cpp".to_string());
        live.model = Some(config.whisper.model.to_string_lossy().to_string());
        live.language = Some(config.whisper.language.clone());
        live.error = None;
        live.dropped_audio_chunks = 0;
    }

    let (audio_tx, audio_rx) = mpsc::sync_channel::<LiveAudioChunk>(LIVE_AUDIO_QUEUE_DEPTH);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let worker_stop_flag = Arc::clone(&stop_flag);
    let join = thread::Builder::new()
        .name("meeting-live-transcript".to_string())
        .spawn(move || {
            run_live_transcript_worker(
                session,
                config,
                live_dir,
                snapshot,
                app,
                audio_rx,
                stop_rx,
                worker_stop_flag,
            );
            let _ = done_tx.send(());
        })
        .map_err(|e| format!("failed to spawn live transcript worker: {e}"))?;

    Ok(LiveTranscriptHandle {
        audio_tx,
        stop_tx,
        done_rx,
        join: Some(join),
        stop_flag,
    })
}

fn run_live_transcript_worker(
    session: MeetingSession,
    config: LiveTranscriptConfig,
    live_dir: PathBuf,
    snapshot: Arc<Mutex<LiveTranscriptSnapshot>>,
    app: Option<AppHandle>,
    audio_rx: mpsc::Receiver<LiveAudioChunk>,
    stop_rx: mpsc::Receiver<()>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut mic = LiveTrackBuffer::new(LiveAudioSource::Mic);
    let mut system = LiveTrackBuffer::new(LiveAudioSource::System);
    let mut chunk_index = 0_u64;
    let mut stop_requested = false;

    while !stop_requested {
        match audio_rx.recv_timeout(config.poll_interval) {
            Ok(chunk) => push_live_audio_chunk(&mut mic, &mut system, chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_requested = true;
            }
        }

        while let Ok(chunk) = audio_rx.try_recv() {
            push_live_audio_chunk(&mut mic, &mut system, chunk);
        }

        if stop_rx.try_recv().is_ok() || stop_flag.load(Ordering::Relaxed) {
            stop_requested = true;
        }

        // On End Meeting, abandon any pending live windows immediately. The
        // authoritative full-file transcription (`run_transcription_job`) re-
        // transcribes the complete WAVs, so draining here is redundant — and
        // continuing would hold the shared whisper process lock for tens of
        // seconds and STARVE that final pass. That was the bug: a meeting
        // finalized off the sparse live transcript (e.g. mic-only "7 words")
        // and needed a manual re-transcribe to recover the real system/video
        // content. Stop now and let the full-file pass run unobstructed.
        if stop_requested {
            break;
        }

        drain_live_ready_windows(
            &session,
            &config,
            &live_dir,
            &snapshot,
            app.as_ref(),
            &mut mic,
            &mut system,
            &mut chunk_index,
            &stop_flag,
            false,
        );
    }

    // NOTE: intentionally NO forced final drain — see above. Draining at stop
    // only races the authoritative full-file transcription for the whisper lock.
    let mut live = snapshot.lock_recover();
    live.running = false;
    if live.status == "running" {
        live.status = "stopped".to_string();
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_live_ready_windows(
    session: &MeetingSession,
    config: &LiveTranscriptConfig,
    live_dir: &Path,
    snapshot: &Arc<Mutex<LiveTranscriptSnapshot>>,
    app: Option<&AppHandle>,
    mic: &mut LiveTrackBuffer,
    system: &mut LiveTrackBuffer,
    chunk_index: &mut u64,
    stop: &AtomicBool,
    force: bool,
) {
    for track in [mic, system] {
        loop {
            // Bail the moment End Meeting is requested — abandon remaining
            // windows so we release the shared whisper lock for the final pass.
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let Some(window) = track.take_ready_window(
                config.context_samples,
                config.step_samples,
                config.min_samples,
                force,
            ) else {
                break;
            };
            match transcribe_live_window(session, config, live_dir, window, *chunk_index) {
                Ok(chunks) => {
                    if !chunks.is_empty() {
                        for chunk in chunks {
                            *chunk_index = chunk_index.saturating_add(1);
                            append_live_transcript_chunk(snapshot, chunk);
                        }
                        // Coalesced delivery: emit ONE event carrying the full
                        // snapshot after a window's chunks are appended — not one
                        // event per chunk. Events ride the JS-eval channel (not the
                        // connection-limited `ipc://` invoke pool), and one event
                        // per ~window (vs N per window) prevents the per-chunk
                        // render flood that saturated the WebView UI thread on
                        // Windows and starved control invokes like End.
                        if let Some(app) = app {
                            let payload = snapshot.lock_recover().payload();
                            emit_main(app, LIVE_TRANSCRIPT_EVENT, payload);
                        }
                    }
                }
                Err(e) => {
                    // "No confident speech" (a silent window, or a track that just
                    // isn't speaking this moment) and a cancelled subprocess are
                    // NORMAL — not errors. Surfacing them flipped the live status
                    // to "running_with_errors" and showed users
                    // "whisper.cpp returned no confident speech transcript"
                    // mid-meeting while the other track transcribed fine. Only
                    // real failures (missing binary, crash, OOM) set the error.
                    let benign =
                        e.contains("no confident speech") || is_cancelled_subprocess_error(&e);
                    if benign {
                        tracing::debug!(error = %e, "[meeting_engine] live window had no confident speech — skipping");
                    } else {
                        tracing::warn!(error = %e, "[meeting_engine] live transcript window failed");
                        let mut live = snapshot.lock_recover();
                        live.error = Some(e);
                        if live.status == "running" {
                            live.status = "running_with_errors".to_string();
                        }
                    }
                }
            }
        }
    }
}

fn push_live_audio_chunk(
    mic: &mut LiveTrackBuffer,
    system: &mut LiveTrackBuffer,
    chunk: LiveAudioChunk,
) {
    match chunk.source {
        LiveAudioSource::Mic => mic.push(chunk.samples),
        LiveAudioSource::System => system.push(chunk.samples),
    }
}

fn append_live_transcript_chunk(
    snapshot: &Arc<Mutex<LiveTranscriptSnapshot>>,
    chunk: MeetingLiveTranscriptChunk,
) {
    // Store only — the coalesced per-window emit in `drain_live_ready_windows`
    // pushes the snapshot to the frontend. (See the comment there.)
    let mut live = snapshot.lock_recover();
    live.chunks.push(chunk);
    live.status = "running".to_string();
    live.error = None;
}

fn transcribe_live_window(
    session: &MeetingSession,
    config: &LiveTranscriptConfig,
    live_dir: &Path,
    window: LiveTranscriptWindow,
    next_chunk_index: u64,
) -> Result<Vec<MeetingLiveTranscriptChunk>, String> {
    let source = window.source;
    let start_ms = window.start_sample.saturating_mul(1_000) / SAMPLE_RATE as u64;
    let emit_from_ms = window
        .emit_from_sample
        .saturating_sub(window.start_sample)
        .saturating_mul(1_000)
        / SAMPLE_RATE as u64;
    let stem = format!("live-{}-{start_ms:010}", source.source_label());
    let wav_path = live_dir.join(format!("{stem}.wav"));
    let summary = write_pcm_window_wav(&wav_path, window.samples)?;
    if !has_transcribable_audio(&summary) {
        return Ok(Vec::new());
    }

    let paths = transcript_paths_for_stem(live_dir, &stem);
    let done = transcribe_with_whisper_cpp_for(
        &summary,
        &paths,
        &config.whisper,
        source.track(),
        config.timeout,
        None,
    )?;
    let segments = label_transcript_segments(
        &done,
        source.source_label(),
        source.speaker_id(),
        source.speaker_name(),
        summary.duration_ms,
    );

    let mut chunks = Vec::new();
    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        if segment.start_ms.saturating_add(500) < emit_from_ms {
            continue;
        }
        let offset = chunks.len() as u64;
        chunks.push(MeetingLiveTranscriptChunk {
            chunk_index: next_chunk_index.saturating_add(offset),
            source: segment.source,
            speaker_id: segment.speaker_id,
            speaker_name: segment.speaker_name,
            timestamp_ms: start_ms.saturating_add(segment.start_ms.max(emit_from_ms)),
            text: text.to_string(),
            is_final: true,
        });
    }

    tracing::info!(
        session_id = %session.session_id,
        source = source.source_label(),
        start_ms,
        chunks = chunks.len(),
        "[meeting_engine] live transcript window completed"
    );

    Ok(chunks)
}

fn write_pcm_window_wav(path: &Path, samples: Vec<i16>) -> Result<MicCaptureSummary, String> {
    let mut writer = create_audio_wav_writer(path, "live transcript")?;
    let mut peak_i16 = 0_i16;
    let mut samples_written = 0_u64;
    for sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| format!("failed to write live transcript sample: {e}"))?;
        samples_written += 1;
        peak_i16 = peak_i16.max(sample.saturating_abs());
    }
    writer
        .finalize()
        .map_err(|e| format!("failed to finalize live transcript WAV: {e}"))?;
    repair_wav_header_sizes(path)?;
    Ok(MicCaptureSummary {
        path: path.to_path_buf(),
        samples_written,
        dropped_chunks: 0,
        native_rate: SAMPLE_RATE,
        duration_ms: samples_written.saturating_mul(1_000) / SAMPLE_RATE as u64,
        peak: peak_i16 as f32 / i16::MAX as f32,
    })
}

fn merge_meeting_audio(
    session: &MeetingSession,
    mic: &MicCaptureSummary,
    system: &SystemCaptureSummary,
) -> Result<MergedMeetingAudio, String> {
    if mic.samples_written == 0 {
        return Err("mic WAV is empty; cannot merge meeting audio".to_string());
    }
    if system.samples_written == 0 {
        return Err("system WAV is empty; cannot merge meeting audio".to_string());
    }

    let merged_path = session.artifact_dir.join("meeting.merged.wav");
    let source_activity_path = session.artifact_dir.join("meeting.source-activity.json");
    let audio_manifest_path = session.artifact_dir.join("meeting.audio.json");
    let mut mic_reader = hound::WavReader::open(&mic.path)
        .map_err(|e| format!("failed to open mic WAV for merge: {e}"))?;
    let mut system_reader = hound::WavReader::open(&system.path)
        .map_err(|e| format!("failed to open system WAV for merge: {e}"))?;
    validate_merge_wav_spec("mic", mic_reader.spec())?;
    validate_merge_wav_spec("system", system_reader.spec())?;

    // Level the two tracks toward a common target so a quiet mic isn't buried
    // under a loud system track in the merged recording.
    let (mic_gain, system_gain) = merge_mix_gains(mic.peak, system.peak);
    tracing::info!(
        mic_peak = mic.peak,
        system_peak = system.peak,
        mic_gain,
        system_gain,
        "[meeting_engine] merging meeting audio with track leveling"
    );

    let mut writer = create_audio_wav_writer(&merged_path, "merged meeting")?;
    let mut mic_samples = mic_reader.samples::<i16>();
    let mut system_samples = system_reader.samples::<i16>();
    let mut samples_written = 0_u64;
    let mut peak_i16 = 0_i16;
    let mut frame_accumulator = SourceActivityAccumulator::new();
    let mut frames = Vec::new();

    loop {
        let mic_sample = next_wav_sample(&mut mic_samples, "mic")?;
        let system_sample = next_wav_sample(&mut system_samples, "system")?;
        if mic_sample.is_none() && system_sample.is_none() {
            break;
        }

        let mic_value = mic_sample.unwrap_or(0);
        let system_value = system_sample.unwrap_or(0);
        let mixed = mix_i16_with_gain(mic_value, system_value, mic_gain, system_gain);
        writer
            .write_sample(mixed)
            .map_err(|e| format!("failed to write merged meeting sample: {e}"))?;
        samples_written += 1;
        peak_i16 = peak_i16.max(mixed.saturating_abs());

        if let Some(frame) = frame_accumulator.push(samples_written - 1, mic_value, system_value) {
            frames.push(frame);
        }
    }

    if let Some(frame) = frame_accumulator.finish() {
        frames.push(frame);
    }

    writer
        .finalize()
        .map_err(|e| format!("failed to finalize merged meeting WAV: {e}"))?;
    repair_wav_header_sizes(&merged_path)?;
    fsync_file(&merged_path);
    let duration_ms = samples_written.saturating_mul(1_000) / SAMPLE_RATE as u64;
    let peak = peak_i16 as f32 / i16::MAX as f32;
    let summary = MicCaptureSummary {
        path: merged_path.clone(),
        samples_written,
        dropped_chunks: mic.dropped_chunks.saturating_add(system.dropped_chunks),
        native_rate: SAMPLE_RATE,
        duration_ms,
        peak,
    };
    let segments = source_activity_segments(&frames);
    write_source_activity_artifact(
        &source_activity_path,
        &audio_manifest_path,
        mic,
        system,
        &summary,
        &segments,
        None,
    );

    Ok(MergedMeetingAudio {
        summary,
        source_activity_path,
    })
}

fn validate_merge_wav_spec(source: &str, spec: hound::WavSpec) -> Result<(), String> {
    if spec.channels != CHANNELS
        || spec.sample_rate != SAMPLE_RATE
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(format!(
            "{source} WAV must be 16 kHz mono 16-bit PCM for merge, got channels={} sample_rate={} bits_per_sample={} format={:?}",
            spec.channels, spec.sample_rate, spec.bits_per_sample, spec.sample_format
        ));
    }
    Ok(())
}

fn next_wav_sample<I>(samples: &mut I, source: &str) -> Result<Option<i16>, String>
where
    I: Iterator<Item = Result<i16, hound::Error>>,
{
    samples
        .next()
        .transpose()
        .map_err(|e| format!("failed to read {source} WAV sample: {e}"))
}

fn mix_i16_samples(mic: i16, system: i16) -> i16 {
    let mixed = mic as i32 + system as i32;
    mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Per-track gains that bring mic and system toward a common playback target,
/// so a quiet mic stays audible next to a loud system track. The mic is only
/// boosted (never cut); a loud system is attenuated down to the target.
fn merge_mix_gains(mic_peak: f32, system_peak: f32) -> (f32, f32) {
    let mic_gain = if mic_peak > ASR_MIN_PEAK_FOR_GAIN {
        (MERGE_MIX_TARGET_PEAK / mic_peak).clamp(1.0, MERGE_MIC_MAX_GAIN)
    } else {
        1.0
    };
    let system_gain = if system_peak > MERGE_MIX_TARGET_PEAK {
        (MERGE_MIX_TARGET_PEAK / system_peak).clamp(0.2, 1.0)
    } else {
        1.0
    };
    (mic_gain, system_gain)
}

fn mix_i16_with_gain(mic: i16, system: i16, mic_gain: f32, system_gain: f32) -> i16 {
    let mixed = mic as f32 * mic_gain + system as f32 * system_gain;
    mixed.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Transliterate transcript segments Devanagari→Roman Hinglish via the (Groq)
/// cleanup LLM, preserving per-segment alignment by numbering the lines. Errors
/// if the response doesn't line up, so the caller can fall back to the
/// deterministic romanizer.
fn transliterate_segments_with_llm(
    segments: &mut [MeetingTranscriptSegment],
    config: MeetingCleanupConfig,
) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }
    let numbered = segments
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.text.replace(['\n', '\r'], " ").trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let completion = complete_meeting_llm(
        MEETING_TRANSLITERATE_SYSTEM_PROMPT,
        &numbered,
        config,
        meeting_ai_timeout(),
        meeting_ai_max_tokens(),
    )?;
    let mut parsed: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for line in completion.content.lines() {
        let line = line.trim();
        if let Some((num, text)) = line.split_once('.') {
            if let Ok(n) = num.trim().parse::<usize>() {
                if n >= 1 {
                    parsed.insert(n - 1, text.trim().to_string());
                }
            }
        }
    }
    // Require most lines back, or the mapping is unreliable → let caller fall back.
    if parsed.len() * 2 < segments.len() {
        return Err(format!(
            "transliteration returned {} of {} lines",
            parsed.len(),
            segments.len()
        ));
    }
    for (i, segment) in segments.iter_mut().enumerate() {
        if let Some(text) = parsed.get(&i) {
            if !text.is_empty() {
                segment.text = text.clone();
            }
        }
        // Backstop: any line the LLM dropped or returned still in Devanagari gets
        // the deterministic romanizer, so the final transcript is never a mix of
        // Roman + Devanagari (the LLM routinely drops a fraction of lines on long
        // inputs).
        if said_core::script::contains_devanagari(&segment.text) {
            segment.text = said_core::script::romanize_devanagari(&segment.text);
        }
    }
    Ok(())
}

struct SourceActivityAccumulator {
    start_sample: u64,
    samples: u64,
    mic_square_sum: f64,
    system_square_sum: f64,
}

impl SourceActivityAccumulator {
    fn new() -> Self {
        Self {
            start_sample: 0,
            samples: 0,
            mic_square_sum: 0.0,
            system_square_sum: 0.0,
        }
    }

    fn push(
        &mut self,
        sample_index: u64,
        mic_sample: i16,
        system_sample: i16,
    ) -> Option<SourceActivityFrame> {
        if self.samples == 0 {
            self.start_sample = sample_index;
        }

        let mic = mic_sample as f64 / 32_768.0;
        let system = system_sample as f64 / 32_768.0;
        self.mic_square_sum += mic * mic;
        self.system_square_sum += system * system;
        self.samples += 1;

        if self.samples >= SOURCE_ACTIVITY_FRAME_SAMPLES {
            self.take_frame(sample_index + 1)
        } else {
            None
        }
    }

    fn finish(&mut self) -> Option<SourceActivityFrame> {
        if self.samples == 0 {
            None
        } else {
            self.take_frame(self.start_sample + self.samples)
        }
    }

    fn take_frame(&mut self, end_sample: u64) -> Option<SourceActivityFrame> {
        if self.samples == 0 {
            return None;
        }
        let frame = SourceActivityFrame {
            start_sample: self.start_sample,
            end_sample,
            mic_rms: (self.mic_square_sum / self.samples as f64).sqrt() as f32,
            system_rms: (self.system_square_sum / self.samples as f64).sqrt() as f32,
        };
        self.samples = 0;
        self.mic_square_sum = 0.0;
        self.system_square_sum = 0.0;
        Some(frame)
    }
}

fn source_activity_segments(frames: &[SourceActivityFrame]) -> Vec<SourceActivitySegment> {
    if frames.is_empty() {
        return Vec::new();
    }

    let max_mic_rms = frames
        .iter()
        .map(|frame| frame.mic_rms)
        .fold(0.0_f32, f32::max);
    let max_system_rms = frames
        .iter()
        .map(|frame| frame.system_rms)
        .fold(0.0_f32, f32::max);
    let mic_threshold =
        SOURCE_ACTIVITY_ABSOLUTE_FLOOR.max(max_mic_rms * SOURCE_ACTIVITY_RELATIVE_FLOOR);
    let system_threshold =
        SOURCE_ACTIVITY_ABSOLUTE_FLOOR.max(max_system_rms * SOURCE_ACTIVITY_RELATIVE_FLOOR);

    let mut segments = Vec::new();
    let mut current: Option<SourceActivitySegment> = None;
    for frame in frames {
        let mic_active = frame.mic_rms >= mic_threshold;
        let system_active = frame.system_rms >= system_threshold;
        let source = match (mic_active, system_active) {
            (true, true) => "overlap",
            (true, false) => "local_mic",
            (false, true) => "system_audio",
            (false, false) => "silence",
        };
        let start_ms = frame.start_sample.saturating_mul(1_000) / SAMPLE_RATE as u64;
        let end_ms = frame.end_sample.saturating_mul(1_000) / SAMPLE_RATE as u64;

        match current.as_mut() {
            Some(segment) if segment.source == source => {
                let old_duration = segment.end_ms.saturating_sub(segment.start_ms).max(1);
                let frame_duration = end_ms.saturating_sub(start_ms).max(1);
                let total_duration = old_duration + frame_duration;
                segment.mic_rms = weighted_average(
                    segment.mic_rms,
                    old_duration,
                    frame.mic_rms,
                    frame_duration,
                    total_duration,
                );
                segment.system_rms = weighted_average(
                    segment.system_rms,
                    old_duration,
                    frame.system_rms,
                    frame_duration,
                    total_duration,
                );
                segment.end_ms = end_ms;
            }
            Some(_) => {
                if let Some(segment) = current.take() {
                    segments.push(segment);
                }
                current = Some(SourceActivitySegment {
                    source: source.to_string(),
                    start_ms,
                    end_ms,
                    mic_rms: frame.mic_rms,
                    system_rms: frame.system_rms,
                });
            }
            None => {
                current = Some(SourceActivitySegment {
                    source: source.to_string(),
                    start_ms,
                    end_ms,
                    mic_rms: frame.mic_rms,
                    system_rms: frame.system_rms,
                });
            }
        }
    }
    if let Some(segment) = current {
        segments.push(segment);
    }
    segments
}

fn weighted_average(
    left_value: f32,
    left_weight: u64,
    right_value: f32,
    right_weight: u64,
    total_weight: u64,
) -> f32 {
    ((left_value as f64 * left_weight as f64 + right_value as f64 * right_weight as f64)
        / total_weight as f64) as f32
}

fn write_source_activity_artifact(
    source_activity_path: &Path,
    audio_manifest_path: &Path,
    mic: &MicCaptureSummary,
    system: &SystemCaptureSummary,
    merged: &MicCaptureSummary,
    segments: &[SourceActivitySegment],
    error: Option<String>,
) {
    let artifact = MeetingAudioArtifact {
        schema_version: 1,
        status: if error.is_some() {
            "failed".to_string()
        } else {
            "completed".to_string()
        },
        mic_wav: mic.path.to_string_lossy().to_string(),
        system_wav: system.path.to_string_lossy().to_string(),
        merged_wav: Some(merged.path.to_string_lossy().to_string()),
        source_activity_path: Some(source_activity_path.to_string_lossy().to_string()),
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        duration_ms: Some(merged.duration_ms),
        samples_written: merged.samples_written,
        source_activity_segments: segments.to_vec(),
        generated_at_ms: now_ms(),
        error: error.clone(),
    };

    match serde_json::to_vec_pretty(&artifact) {
        Ok(json) => {
            if let Err(e) = write_atomic(audio_manifest_path, &json) {
                tracing::warn!(error = %e, path = %audio_manifest_path.display(), "[meeting_engine] failed to write audio manifest json");
            }
            if let Err(e) = write_atomic(source_activity_path, json) {
                tracing::warn!(error = %e, path = %source_activity_path.display(), "[meeting_engine] failed to write source activity json");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize source activity json");
        }
    }
}

/// Durably write `contents` to `path` via a sibling temp file + atomic rename,
/// so a crash mid-write can never leave a truncated or half-serialized artifact
/// on disk — readers either see the previous complete file or the new one. The
/// temp file lives next to the target so the rename stays on one filesystem.
fn write_atomic(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    // Persist the rename itself: without fsyncing the parent directory, a power
    // loss after rename can lose the directory entry even though the temp file's
    // data was synced.
    if let Some(parent) = path.parent() {
        fsync_dir(parent);
    }
    Ok(())
}

/// Force an already-written file's contents to stable storage. Best-effort —
/// the data is more valuable than the latency, but a failed fsync only warns
/// (the write already succeeded into the page cache).
fn fsync_file(path: &Path) {
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => {
            if let Err(e) = file.sync_all() {
                tracing::warn!(error = %e, path = %path.display(), "[meeting_engine] fsync failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "[meeting_engine] fsync open failed");
        }
    }
}

/// Force a directory entry (e.g. after a create/rename) to stable storage.
/// No-op on Windows, where directory handles can't be fsynced this way.
fn fsync_dir(dir: &Path) {
    #[cfg(not(windows))]
    if let Ok(file) = File::open(dir) {
        let _ = file.sync_all();
    }
    #[cfg(windows)]
    let _ = dir;
}

/// Name of the per-meeting checkpoint file written beside the audio/transcript
/// artifacts. Startup recovery reads it to learn how far processing got before
/// the app last exited.
const MEETING_STATE_FILE: &str = "meeting.state.json";

/// Coarse processing phase for a meeting's artifact directory. Persisted to
/// [`MEETING_STATE_FILE`] at each lifecycle transition so a crash leaves a
/// breadcrumb for recovery. `Recording` and `Transcribing` are non-terminal:
/// finding one on startup means the app died mid-pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MeetingProcessingState {
    phase: String,
    updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

const MEETING_PHASE_RECORDING: &str = "recording";
const MEETING_PHASE_TRANSCRIBING: &str = "transcribing";
const MEETING_PHASE_TRANSCRIBED: &str = "transcribed";
const MEETING_PHASE_SUMMARIZED: &str = "summarized";
const MEETING_PHASE_FAILED: &str = "failed";
const MEETING_PHASE_CANCELLED: &str = "cancelled";

fn write_meeting_state(artifact_dir: &Path, phase: &str, error: Option<String>) {
    if !artifact_dir.is_dir() {
        return;
    }
    let state = MeetingProcessingState {
        phase: phase.to_string(),
        updated_at_ms: now_ms(),
        error,
    };
    match serde_json::to_vec_pretty(&state) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(artifact_dir.join(MEETING_STATE_FILE), bytes) {
                tracing::warn!(error = %e, dir = %artifact_dir.display(), "[meeting_engine] failed to write meeting state");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize meeting state");
        }
    }
}

fn read_meeting_state(artifact_dir: &Path) -> Option<MeetingProcessingState> {
    let bytes = fs::read(artifact_dir.join(MEETING_STATE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// True when a meeting folder holds a *usable* transcript (completed, non-empty),
/// as opposed to a `failed`/`skipped` artifact.
fn meeting_has_usable_transcript(dir: &Path) -> bool {
    if dir.join("meeting.transcript.final.json").is_file() {
        return true;
    }
    let Ok(bytes) = fs::read(dir.join("meeting.transcript.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let transcript = value
        .get("transcript")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    status == "completed" && !transcript.trim().is_empty()
}

/// Per-meeting processing status the frontend polls to render live stage progress
/// (transcribing → cleaning → summarizing → done) and a Retry affordance. Read
/// from disk (so it survives restarts and works for background jobs) and overlaid
/// with the live queue/worker state.
#[derive(Debug, Serialize)]
pub struct MeetingProcessingStatusPayload {
    pub meeting_id: String,
    pub phase: String,
    pub stage: String,
    pub running: bool,
    pub queued: bool,
    pub cancelling: bool,
    pub can_cancel: bool,
    pub can_retry: bool,
    pub error: Option<String>,
    pub progress: Option<MeetingProcessingProgress>,
    pub has_transcript: bool,
    pub has_intelligence: bool,
    /// Transcript is done but the summary stage failed (recoverable via
    /// regenerate, distinct from a transcription failure that needs re-transcribe).
    pub summary_failed: bool,
    pub updated_at_ms: u64,
}

// Async (off the main thread): meeting_processing_status locks state.jobs and
// state.transcription — mutexes the live recorder/transcription threads hold for
// long stretches. As a SYNC command this ran on the main thread, so when the
// Meetings list opened a still-recording meeting it blocked the main thread on
// those locks → IPC dispatch stalled → "End" (stop_session) could never dispatch
// → the meeting never stopped → the lock never freed → permanent deadlock. Off
// the main thread, dispatch stays free so End always lands.
#[tauri::command]
pub fn meeting_engine_get_processing_status(
    state: State<'_, MeetingEngineState>,
    meeting_id: String,
) -> Result<MeetingProcessingStatusPayload, String> {
    meeting_processing_status(&state, meeting_id)
}

fn meeting_processing_status(
    state: &MeetingEngineState,
    meeting_id: String,
) -> Result<MeetingProcessingStatusPayload, String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    let disk = read_meeting_state(&dir);
    let phase = disk
        .as_ref()
        .map(|s| s.phase.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let mut error = disk.as_ref().and_then(|s| s.error.clone());
    let updated_at_ms = disk.as_ref().map(|s| s.updated_at_ms).unwrap_or(0);

    let has_transcript = meeting_has_usable_transcript(&dir);
    let has_intelligence = dir.join("meeting.ai.json").is_file();

    // Live overlay: is this meeting in-flight on the worker, or merely queued?
    let (in_flight, queued, cancelling) = {
        let inner = state.jobs.lock();
        let in_flight = inner.in_flight.as_deref() == Some(meeting_id.as_str());
        let queued = !in_flight && inner.pending.iter().any(|j| j.meeting_id == meeting_id);
        let cancelling = inner.cancelled.contains(&meeting_id);
        (in_flight, queued, cancelling)
    };
    let running = in_flight || queued;

    // Fine-grained stage. The global transcription snapshot only describes the
    // in-flight meeting, so we only trust it when THIS meeting is in flight.
    let mut progress = None;
    let stage = if running && cancelling {
        "cancelling".to_string()
    } else if in_flight {
        let snapshot = state.transcription.lock_recover();
        progress = snapshot.progress.clone();
        match snapshot.status.as_str() {
            "running" | "" => "transcribing",
            "cleaning" => "cleaning",
            "completed" | "diarizing" | "final_diarizing" => "summarizing",
            "summarizing" => "summarizing",
            other => other,
        }
        .to_string()
    } else if queued {
        "queued".to_string()
    } else {
        phase.clone()
    };

    // Summary stage failed: transcript is on disk but no intelligence cache and a
    // recorded error. Recoverable by regenerating the summary (not re-transcribing).
    let summary_failed = !running
        && has_transcript
        && !has_intelligence
        && error
            .as_deref()
            .is_some_and(|e| e.to_ascii_lowercase().contains("summary"));

    let stage = if !running && summary_failed {
        "summary_failed".to_string()
    } else {
        stage
    };
    if stage != "transcribing" {
        progress = None;
    }

    if running {
        error = None;
    }

    let terminal_phase = matches!(
        phase.as_str(),
        MEETING_PHASE_TRANSCRIBED
            | MEETING_PHASE_SUMMARIZED
            | MEETING_PHASE_FAILED
            | MEETING_PHASE_CANCELLED
    );
    let has_audio = RECOVERABLE_MEETING_WAVS
        .iter()
        .any(|name| dir.join(name).is_file());
    // Retry is offered when nothing is running for this meeting, audio exists, and
    // either it failed or it's stuck in a non-terminal phase without a transcript
    // (interrupted before completion). Summary-stage failures are retried via the
    // separate regenerate path, not here.
    let can_retry = !running
        && has_audio
        && (phase == MEETING_PHASE_FAILED
            || phase == MEETING_PHASE_CANCELLED
            || (!terminal_phase && !has_transcript));

    Ok(MeetingProcessingStatusPayload {
        meeting_id,
        phase,
        stage,
        running,
        queued,
        cancelling,
        can_cancel: running && !cancelling,
        can_retry,
        error,
        progress,
        has_transcript,
        has_intelligence,
        summary_failed,
        updated_at_ms,
    })
}

fn repair_wav_header_sizes(path: &Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("failed to open WAV for header check: {e}"))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("failed to stat WAV: {e}"))?
        .len();
    if file_len < 44 {
        return Err("WAV is too small to contain a valid header".to_string());
    }
    if file_len > u32::MAX as u64 + 8 {
        return Err("WAV is too large for a RIFF header".to_string());
    }

    let mut header = [0_u8; 44];
    file.read_exact(&mut header)
        .map_err(|e| format!("failed to read WAV header: {e}"))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("WAV header is missing RIFF/WAVE markers".to_string());
    }
    if &header[36..40] != b"data" {
        return Ok(());
    }

    let expected_riff_size = (file_len - 8) as u32;
    let expected_data_size = (file_len - 44) as u32;
    let current_riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let current_data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    if current_riff_size == expected_riff_size && current_data_size == expected_data_size {
        return Ok(());
    }

    file.seek(SeekFrom::Start(4))
        .map_err(|e| format!("failed to seek WAV RIFF size: {e}"))?;
    file.write_all(&expected_riff_size.to_le_bytes())
        .map_err(|e| format!("failed to repair WAV RIFF size: {e}"))?;
    file.seek(SeekFrom::Start(40))
        .map_err(|e| format!("failed to seek WAV data size: {e}"))?;
    file.write_all(&expected_data_size.to_le_bytes())
        .map_err(|e| format!("failed to repair WAV data size: {e}"))?;
    file.flush()
        .map_err(|e| format!("failed to flush repaired WAV header: {e}"))?;
    Ok(())
}

/// WAV files a meeting may leave on disk, in the order recovery prefers them.
const RECOVERABLE_MEETING_WAVS: [&str; 3] = ["meeting.merged.wav", "mic.wav", "system.wav"];

/// Remove a meeting dir that holds no audio and no usable transcript — i.e. an
/// empty placeholder left by a session that captured nothing. Defensive: refuses
/// to delete anything that still has recoverable audio or a real transcript.
/// Cloud meeting ids whose local artifacts were discarded as empty (immediate
/// stop, denied mic, silence, or a session killed mid-recording). The desktop
/// drains this on the next meetings refresh and deletes the matching cloud
/// records, so an abandoned meeting leaves nothing behind — not the local dir,
/// not the server "Quick meeting". `local-…` placeholders never reach the cloud,
/// so they are removed silently and not recorded here.
static DISCARDED_CLOUD_MEETING_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Record a removed empty dir for cloud cleanup when it was a cloud-backed
/// meeting (a real id, not a `local-…` placeholder).
fn record_discarded_cloud_meeting(dir: &Path) {
    let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    if id.starts_with("local-") {
        return;
    }
    if let Ok(mut ids) = DISCARDED_CLOUD_MEETING_IDS.lock() {
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
}

/// Drain the ids of empty meetings whose local artifacts were discarded. The
/// desktop calls this and deletes the matching cloud records so empty meetings
/// never linger server-side. Idempotent: returns each id once.
#[tauri::command]
pub fn meeting_engine_take_discarded_meeting_ids() -> Vec<String> {
    DISCARDED_CLOUD_MEETING_IDS
        .lock()
        .map(|mut ids| std::mem::take(&mut *ids))
        .unwrap_or_default()
}

fn cleanup_empty_session_dir(dir: &Path) {
    let has_audio = RECOVERABLE_MEETING_WAVS
        .iter()
        .any(|name| dir.join(name).is_file());
    if has_audio || meeting_has_usable_transcript(dir) {
        return;
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => {
            record_discarded_cloud_meeting(dir);
            tracing::info!(dir = %dir.display(), "[meeting_engine] removed empty meeting dir (no audio captured)")
        }
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "[meeting_engine] failed to remove empty meeting dir")
        }
    }
}

/// Delete the disposable intermediates once a meeting has a final transcript:
/// the per-window `live/` WAVs (+ their whisper sidecars), final-ASR chunk dirs,
/// and the `*.asr.wav` gain-normalized copies. These are only needed during
/// transcription and are the bulk of a meeting's disk footprint (hundreds of
/// files / hundreds of MB on a long meeting). The source `mic.wav`/`system.wav`
/// and the transcript/summary artifacts are kept.
fn prune_meeting_intermediates(dir: &Path) {
    let live_dir = dir.join("live");
    if live_dir.is_dir() {
        if let Err(e) = fs::remove_dir_all(&live_dir) {
            tracing::warn!(error = %e, dir = %live_dir.display(), "[meeting_engine] failed to prune live/ windows");
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".asr.wav") {
                let _ = fs::remove_file(&path);
            } else if name.ends_with(".asr-chunks") && path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}

/// Startup storage GC, two passes over the meetings root:
///  1. Remove `local-*` orphan dirs with no audio and no usable transcript —
///     placeholders from sessions that captured nothing (immediate stop / denied
///     mic), invisible in the server-driven UI so nothing else reclaims them.
///     Server-UUID dirs are never removed.
///  2. For any meeting that already has a usable transcript, prune its disposable
///     intermediates (live/ windows + final ASR copies/chunks). This also
///     reclaims the intermediates left by meetings that completed before pruning
///     existed.
/// Runs at startup only, when no session is active, so it can't race a live one.
pub fn gc_orphan_meeting_dirs() {
    let root = said_core::paths::data_dir().join("meetings");
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let mut removed = 0_u32;
    let mut pruned = 0_u32;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let has_audio = RECOVERABLE_MEETING_WAVS
            .iter()
            .any(|name| dir.join(name).is_file());
        let has_transcript = meeting_has_usable_transcript(&dir);

        // Any empty meeting dir (no audio, no transcript) is junk — a meeting
        // that was abandoned/killed before capturing anything. Reclaim it. For a
        // cloud-backed id this also records the meeting for cloud-record deletion
        // (record_discarded_cloud_meeting ignores `local-…` placeholders), so a
        // crash mid-recording never leaves an orphan "Quick meeting" server-side.
        if !has_audio && !has_transcript {
            if fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
                record_discarded_cloud_meeting(&dir);
            }
            continue;
        }

        if has_transcript && (dir.join("live").is_dir() || dir_has_asr_copy(&dir)) {
            prune_meeting_intermediates(&dir);
            pruned += 1;
        }
    }
    if removed > 0 || pruned > 0 {
        tracing::info!(
            removed,
            pruned,
            "[meeting_engine] startup meeting storage GC complete"
        );
    }
}

fn dir_has_asr_copy(dir: &Path) -> bool {
    fs::read_dir(dir).ok().is_some_and(|entries| {
        entries.flatten().any(|e| {
            let path = e.path();
            let name = e.file_name();
            name.to_str().is_some_and(|n| {
                n.ends_with(".asr.wav") || (n.ends_with(".asr-chunks") && path.is_dir())
            })
        })
    })
}

/// Best-effort startup recovery pass. Scans every meeting artifact directory and,
/// for any meeting that did not reach a terminal phase before the app exited,
/// repairs its WAV header sizes — a crash leaves the `BufWriter`-backed WAV with
/// stale RIFF/data sizes that make the file unplayable until rewritten. Repair is
/// idempotent (a no-op once sizes already match the file length), so completed
/// meetings are untouched.
///
/// This makes interrupted recordings playable again on next launch. Regenerating
/// the transcript/summary for an interrupted meeting is handled separately (it
/// must serialize through the engine's single transcription slot).
///
/// Never panics; a bad directory is logged and skipped.
pub fn recover_incomplete_meetings() {
    let root = said_core::paths::data_dir().join("meetings");
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let mut recovered = 0_u32;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let phase = read_meeting_state(&dir).map(|state| state.phase);
        match phase.as_deref() {
            // Terminal phases are fully processed — nothing to do.
            Some(
                MEETING_PHASE_TRANSCRIBED
                | MEETING_PHASE_SUMMARIZED
                | MEETING_PHASE_FAILED
                | MEETING_PHASE_CANCELLED,
            ) => continue,
            // Legacy meeting (no checkpoint file): skip if it already has a final
            // transcript, otherwise fall through and at least repair its audio.
            None if dir.join("meeting.transcript.final.json").is_file()
                || dir.join("meeting.transcript.final.txt").is_file() =>
            {
                continue;
            }
            _ => {}
        }

        let wavs: Vec<PathBuf> = RECOVERABLE_MEETING_WAVS
            .iter()
            .map(|name| dir.join(name))
            .filter(|path| path.is_file())
            .collect();
        if wavs.is_empty() {
            continue;
        }

        let mut repaired = false;
        for wav in &wavs {
            match repair_wav_header_sizes(wav) {
                Ok(()) => repaired = true,
                Err(e) => {
                    tracing::warn!(error = %e, path = %wav.display(), "[meeting_engine] recovery WAV repair failed");
                }
            }
        }
        if repaired {
            recovered += 1;
            tracing::info!(dir = %dir.display(), phase = ?phase, "[meeting_engine] recovered interrupted meeting audio");
        }
    }
    if recovered > 0 {
        tracing::info!(
            count = recovered,
            "[meeting_engine] startup meeting recovery complete"
        );
    }
}

fn transcript_paths_for_wav(wav_path: &Path) -> TranscriptPaths {
    let parent = wav_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = wav_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("mic");
    transcript_paths_for_stem(parent, stem)
}

fn transcript_paths_for_stem(parent: &Path, stem: &str) -> TranscriptPaths {
    let whisper_out_base = parent.join(format!("{stem}.whisper"));
    let whisper_txt = parent.join(format!("{stem}.whisper.txt"));
    let whisper_json = parent.join(format!("{stem}.whisper.json"));
    TranscriptPaths {
        text: parent.join(format!("{stem}.transcript.txt")),
        json: parent.join(format!("{stem}.transcript.json")),
        whisper_txt,
        whisper_json,
        whisper_out_base,
    }
}

/// Reconstruct a capture summary from a saved WAV (for re-transcription) by
/// reading its frame count and peak amplitude off disk.
fn capture_summary_from_wav(path: &Path) -> Option<MicCaptureSummary> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let mut samples: u64 = 0;
    let mut peak: i32 = 0;
    for sample in reader.into_samples::<i16>() {
        let value = sample.ok()?;
        samples += 1;
        let amp = (value as i32).abs();
        if amp > peak {
            peak = amp;
        }
    }
    if samples == 0 {
        return None;
    }
    let rate = spec.sample_rate.max(1) as u64;
    Some(MicCaptureSummary {
        path: path.to_path_buf(),
        samples_written: samples,
        dropped_chunks: 0,
        native_rate: spec.sample_rate,
        duration_ms: samples * 1000 / rate,
        peak: peak as f32 / i16::MAX as f32,
    })
}

/// Build a transcription plan from a meeting folder's saved audio so it can be
/// re-transcribed with the current language/model settings. Prefers separate
/// mic + system tracks (matching the live pipeline); falls back to the merged
/// track when the per-source WAVs are gone.
fn build_retranscribe_plan(dir: &Path) -> Result<MeetingTranscriptionPlan, String> {
    // Repair WAV size headers before reading. A recording interrupted before its
    // graceful finalize (crash, force-quit, hard kill) leaves the RIFF/`data`
    // chunk sizes at 0, so the file would read as 0 samples and be wrongly
    // skipped as "empty" — losing a recording that is fully present on disk.
    // `repair_wav_header_sizes` is idempotent, so already-finalized files are
    // untouched.
    for name in RECOVERABLE_MEETING_WAVS {
        let wav = dir.join(name);
        if wav.is_file() {
            if let Err(e) = repair_wav_header_sizes(&wav) {
                tracing::warn!(error = %e, path = %wav.display(), "[meeting_engine] retranscribe WAV header repair failed");
            }
        }
    }

    let mic = capture_summary_from_wav(&dir.join("mic.wav"));
    let system = capture_summary_from_wav(&dir.join("system.wav"));
    let merged = capture_summary_from_wav(&dir.join("meeting.merged.wav"));

    let mic_summary = mic.clone().or_else(|| merged.clone()).ok_or_else(|| {
        "no audio found to re-transcribe (expected mic.wav or meeting.merged.wav)".to_string()
    })?;
    let summary = merged.clone().unwrap_or_else(|| mic_summary.clone());

    let mut source_wavs = Vec::new();
    if mic.is_some() {
        source_wavs.push(dir.join("mic.wav"));
    }
    if system.is_some() {
        source_wavs.push(dir.join("system.wav"));
    }
    if source_wavs.is_empty() {
        source_wavs.push(mic_summary.path.clone());
    }

    Ok(MeetingTranscriptionPlan {
        mic: mic_summary,
        system,
        summary,
        output_paths: transcript_paths_for_stem(dir, "meeting"),
        source_wavs,
        source_activity_path: Some(dir.join("meeting.source-activity.json"))
            .filter(|path| path.is_file()),
    })
}

/// Scan a 16-bit PCM mono WAV once and return its (peak, rms) in linear 0.0–1.0.
/// RMS is the loudness measure we gate and normalize on; peak is the anti-clip
/// limiter. Returns (0.0, 0.0) for an empty file.
fn analyze_wav_levels(path: &Path) -> Result<(f32, f32), String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("failed to open WAV for level analysis: {e}"))?;
    let mut peak_i: i32 = 0;
    let mut sum_sq: f64 = 0.0;
    let mut count: u64 = 0;
    for sample in reader.samples::<i16>() {
        let s = sample.map_err(|e| format!("failed to read WAV sample for level analysis: {e}"))?;
        let abs = (s as i32).abs();
        if abs > peak_i {
            peak_i = abs;
        }
        let f = s as f64;
        sum_sq += f * f;
        count += 1;
    }
    if count == 0 {
        return Ok((0.0, 0.0));
    }
    let peak = peak_i as f32 / i16::MAX as f32;
    let rms = ((sum_sq / count as f64).sqrt() as f32) / i16::MAX as f32;
    Ok((peak, rms))
}

fn prepare_whisper_audio_input(summary: &MicCaptureSummary) -> Result<PathBuf, String> {
    // Measure true loudness (RMS), not just the capture peak. Peak-only
    // normalization amplifies a noise-dominated track's floor to full scale and
    // induces hallucination; RMS targeting preserves SNR.
    let gain = match analyze_wav_levels(&summary.path) {
        Ok((peak, rms)) if rms > 0.0 => asr_gain_for_levels(peak, rms),
        Ok(_) => 1.0,
        Err(e) => {
            // Couldn't read levels — fall back to the legacy peak-based gain
            // (bounded by the lowered ASR_MAX_GAIN) so recovery still works.
            tracing::warn!(error = %e, "[meeting_engine] WAV level analysis failed; using capture-peak gain");
            asr_gain_for_peak(summary.peak)
        }
    };
    if gain <= 1.01 {
        return Ok(summary.path.clone());
    }

    let parent = summary.path.parent().unwrap_or_else(|| Path::new("."));
    let stem = summary
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("audio");
    let asr_path = parent.join(format!("{stem}.asr.wav"));
    normalize_wav_for_asr(&summary.path, &asr_path, gain)?;
    Ok(asr_path)
}

/// RMS-targeted, peak-limited, bounded ASR gain. Targets conversational loudness
/// (ASR_TARGET_RMS) but never pushes the peak past the clip ceiling, and is hard
/// capped so we don't amplify a quiet track's noise floor into hallucination.
fn asr_gain_for_levels(peak: f32, rms: f32) -> f32 {
    let max_gain = env_f32("AIRNOTE_MEETING_ASR_MAX_GAIN", ASR_MAX_GAIN).max(1.0);
    let target_rms = env_f32("AIRNOTE_MEETING_ASR_TARGET_RMS", ASR_TARGET_RMS);
    if !peak.is_finite() || !rms.is_finite() || peak <= ASR_MIN_PEAK_FOR_GAIN || rms <= 0.0 {
        return 1.0;
    }
    let rms_gain = target_rms / rms; // reach conversational loudness…
    let peak_gain = ASR_TARGET_PEAK / peak; // …without clipping.
    rms_gain.min(peak_gain).clamp(1.0, max_gain)
}

/// Legacy peak-only gain — used only as a fallback when RMS can't be measured.
fn asr_gain_for_peak(peak: f32) -> f32 {
    let max_gain = env_f32("AIRNOTE_MEETING_ASR_MAX_GAIN", ASR_MAX_GAIN).max(1.0);
    if !peak.is_finite() || !(ASR_MIN_PEAK_FOR_GAIN..ASR_TARGET_PEAK).contains(&peak) {
        return 1.0;
    }
    (ASR_TARGET_PEAK / peak).clamp(1.0, max_gain)
}

fn has_transcribable_audio(summary: &MicCaptureSummary) -> bool {
    if summary.samples_written == 0 || !summary.peak.is_finite() {
        return false;
    }
    // Fast reject: no meaningful transient at all.
    if summary.peak < ASR_MIN_PEAK_FOR_TRANSCRIPTION {
        return false;
    }
    // Loudness (RMS) gate — the real silence-guard. A track can have a high
    // transient peak (a bleed spike, a click) yet be silence on average; forcing
    // ASR on it, especially with a forced language, produces hallucinated text.
    // Require genuine speech energy. If the scan fails, fall back to the peak
    // check above (don't silently drop a real recording over an I/O hiccup).
    let min_rms = env_f32("AIRNOTE_MEETING_ASR_MIN_RMS", ASR_MIN_RMS_FOR_TRANSCRIPTION);
    match analyze_wav_levels(&summary.path) {
        Ok((_, rms)) if rms < min_rms => {
            tracing::warn!(
                peak = summary.peak,
                rms,
                min_rms,
                "[meeting_engine] track RMS below speech floor; treating as silence (skipping ASR to avoid hallucination)"
            );
            false
        }
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] RMS gate scan failed; falling back to peak gate");
            true
        }
    }
}

fn normalize_wav_for_asr(input: &Path, output: &Path, gain: f32) -> Result<(), String> {
    let mut reader = hound::WavReader::open(input)
        .map_err(|e| format!("failed to open WAV for ASR normalization: {e}"))?;
    validate_merge_wav_spec("ASR input", reader.spec())?;
    let mut writer = create_audio_wav_writer(output, "ASR normalized")?;
    for sample in reader.samples::<i16>() {
        let sample =
            sample.map_err(|e| format!("failed to read WAV sample for ASR normalization: {e}"))?;
        let boosted = ((sample as f32) * gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer
            .write_sample(boosted)
            .map_err(|e| format!("failed to write ASR normalized sample: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("failed to finalize ASR normalized WAV: {e}"))?;
    repair_wav_header_sizes(output)?;
    Ok(())
}

fn whisper_language_for_track(_track: MeetingAudioTrack, default_language: &str) -> String {
    default_language.to_string()
}

fn whisper_translate_for_track(track: MeetingAudioTrack) -> bool {
    match track {
        MeetingAudioTrack::Mic => env_bool("AIRNOTE_MEETING_MIC_WHISPER_TRANSLATE", false),
        MeetingAudioTrack::System => env_bool("AIRNOTE_MEETING_SYSTEM_WHISPER_TRANSLATE", false),
    }
}

/// Parse whisper.cpp's `--detect-language` output into an en-or-hi decision.
/// Whisper's raw auto-detect mislabels Hindi as Urdu/Indonesian/Korean on short
/// audio, so we trust ONLY a confident English detection; everything else falls
/// back to Hindi (the Hinglish-first default). Validated on real recordings.
fn route_detected_meeting_language(detect_output: &str) -> String {
    let needle = "auto-detected language:";
    if let Some(idx) = detect_output.find(needle) {
        let rest = &detect_output[idx + needle.len()..];
        let lang = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let prob = rest
            .split("p =")
            .nth(1)
            .map(|s| {
                s.trim()
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
            })
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.0);
        if lang == "en" && prob >= 0.5 {
            return "en".to_string();
        }
    }
    DEFAULT_WHISPER_LANGUAGE.to_string()
}

/// Detect a single meeting track's language via a fast whisper.cpp `-dl` pass.
fn detect_meeting_track_language(config: &WhisperCppConfig, audio_path: &Path) -> String {
    let mut cmd = Command::new(&config.binary);
    // NOTE: do NOT pass -np here — it suppresses whisper.cpp's
    // "auto-detected language: xx (p=…)" line that we parse, which would make
    // detection silently always fall back to Hindi.
    cmd.arg("-m")
        .arg(&config.model)
        .arg("-f")
        .arg(audio_path)
        .arg("-dl")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_meeting_child_console(&mut cmd);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] track language detect spawn failed; using default");
            return config.language.clone();
        }
    };
    match wait_with_timeout(child, Duration::from_secs(45), None) {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            route_detected_meeting_language(&text)
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] track language detect failed; using default");
            config.language.clone()
        }
    }
}

/// Build a per-track whisper config whose language is auto-detected (en-or-hi).
/// This stops the mic track (and any English participant/share) from being forced
/// into Hindi — which produced Devanagari garbage that the gates rejected as
/// "no confident speech". Set AIRNOTE_MEETING_LANG_AUTODETECT=0 to force the
/// default language for every track.
fn meeting_track_config(config: &WhisperCppConfig, audio_path: &Path) -> WhisperCppConfig {
    if !env_bool("AIRNOTE_MEETING_LANG_AUTODETECT", true) {
        return config.clone();
    }
    let mut per_track = config.clone();
    per_track.language = detect_meeting_track_language(config, audio_path);
    tracing::info!(
        track = %audio_path.display(),
        language = %per_track.language,
        "[meeting_engine] per-track language routed"
    );
    per_track
}

fn transcribe_meeting_plan(
    plan: &MeetingTranscriptionPlan,
    config: &WhisperCppConfig,
    cancel_check: Option<&dyn Fn() -> bool>,
    progress: Option<&dyn Fn(MeetingProcessingProgress)>,
) -> Result<MeetingPlanTranscriptionDone, String> {
    let started = Instant::now();
    let mic_paths = transcript_paths_for_wav(&plan.mic.path);
    let chunk_ms = final_asr_chunk_ms();
    let mic_chunk_count = if may_have_final_asr_work(&plan.mic) {
        final_asr_chunk_count(&plan.mic, chunk_ms)
    } else {
        0
    };
    let system_chunk_count = plan
        .system
        .as_ref()
        .filter(|summary| may_have_final_asr_work(summary))
        .map(|summary| final_asr_chunk_count(summary, chunk_ms))
        .unwrap_or(0);
    let total_chunks = if plan.system.is_some() {
        mic_chunk_count.saturating_add(system_chunk_count).max(1)
    } else {
        mic_chunk_count.max(1)
    };
    let completed_chunks = Cell::new(0_u64);
    let report_chunk_progress = |chunk: WhisperChunkProgress| {
        if let Some(progress) = progress {
            let current = completed_chunks
                .get()
                .saturating_add(chunk.current)
                .min(total_chunks)
                .max(1);
            progress(MeetingProcessingProgress::transcribing(
                current,
                total_chunks.max(chunk.total),
                chunk.track,
            ));
        }
    };

    let Some(system_summary) = &plan.system else {
        if !has_transcribable_audio(&plan.mic) {
            return Err(format!(
                "mic track peak {:.6} is below speech threshold; skipping transcription",
                plan.mic.peak
            ));
        }
        let mic_config = meeting_track_config(config, &plan.mic.path);
        let mic_done = transcribe_with_whisper_cpp(
            &plan.mic,
            &mic_paths,
            &mic_config,
            MeetingAudioTrack::Mic,
            cancel_check,
            Some(&report_chunk_progress),
        )?;
        let segments = label_transcript_segments(
            &mic_done,
            transcript_source(&plan.mic.path),
            transcript_source_id(&plan.mic.path),
            transcript_source_name(&plan.mic.path),
            plan.mic.duration_ms,
        );
        return Ok(MeetingPlanTranscriptionDone {
            transcript: mic_done.transcript,
            latency_ms: mic_done.latency_ms,
            summary: plan.summary.clone(),
            segments,
            source_wavs: plan.source_wavs.clone(),
        });
    };

    let mic_result = if has_transcribable_audio(&plan.mic) {
        let mic_config = meeting_track_config(config, &plan.mic.path);
        Some(transcribe_with_whisper_cpp(
            &plan.mic,
            &mic_paths,
            &mic_config,
            MeetingAudioTrack::Mic,
            cancel_check,
            Some(&report_chunk_progress),
        ))
    } else {
        tracing::warn!(
            peak = plan.mic.peak,
            samples_written = plan.mic.samples_written,
            "[meeting_engine] mic audio below speech threshold; skipping mic ASR"
        );
        None
    };
    if matches!(mic_result, Some(Ok(_))) {
        completed_chunks.set(completed_chunks.get().saturating_add(mic_chunk_count));
    }
    let system_paths = transcript_paths_for_wav(&system_summary.path);
    let system_result = if has_transcribable_audio(system_summary) {
        let system_config = meeting_track_config(config, &system_summary.path);
        Some(transcribe_with_whisper_cpp(
            system_summary,
            &system_paths,
            &system_config,
            MeetingAudioTrack::System,
            cancel_check,
            Some(&report_chunk_progress),
        ))
    } else {
        tracing::warn!(
            peak = system_summary.peak,
            samples_written = system_summary.samples_written,
            "[meeting_engine] system audio below speech threshold; skipping system ASR"
        );
        None
    };
    let mut errors = Vec::new();
    let mut segments = Vec::new();
    match mic_result {
        None => {
            errors.push("mic: below speech threshold".to_string());
        }
        Some(Ok(done)) => {
            segments.extend(label_transcript_segments(
                &done,
                "mic",
                "you",
                "You",
                plan.mic.duration_ms,
            ));
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "[meeting_engine] mic track transcription failed in dual-track plan");
            errors.push(format!("mic: {e}"));
        }
    }
    match system_result {
        None => {
            errors.push("system: below speech threshold".to_string());
        }
        Some(Ok(done)) => {
            segments.extend(label_transcript_segments(
                &done,
                "system",
                "speaker_1",
                "Speaker 1",
                system_summary.duration_ms,
            ));
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "[meeting_engine] system track transcription failed in dual-track plan");
            errors.push(format!("system: {e}"));
        }
    }
    if segments.is_empty() {
        return Err(format!(
            "both meeting tracks failed transcription: {}",
            errors.join("; ")
        ));
    }
    segments.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| {
                source_sort_key(&left.speaker_id).cmp(&source_sort_key(&right.speaker_id))
            })
            .then_with(|| left.end_ms.cmp(&right.end_ms))
    });
    segments = suppress_mic_echo_segments(segments, plan.source_activity_path.as_deref());

    let transcript = format_meeting_timeline_transcript(&segments);
    write_atomic(&plan.output_paths.text, transcript.as_bytes())
        .map_err(|e| format!("failed to write meeting timeline transcript text: {e}"))?;

    Ok(MeetingPlanTranscriptionDone {
        transcript,
        latency_ms: started.elapsed().as_millis() as u64,
        summary: plan.summary.clone(),
        segments: segments.clone(),
        source_wavs: plan.source_wavs.clone(),
    })
}

fn label_transcript_segments(
    done: &WhisperTranscriptionDone,
    source: &str,
    speaker_id: &str,
    speaker_name: &str,
    duration_ms: u64,
) -> Vec<MeetingTranscriptSegment> {
    let raw_segments = if done.segments.is_empty() && !done.transcript.trim().is_empty() {
        vec![RawTranscriptSegment {
            start_ms: 0,
            end_ms: duration_ms,
            text: done.transcript.clone(),
        }]
    } else {
        done.segments.clone()
    };

    raw_segments
        .into_iter()
        .filter_map(|segment| {
            let text = segment.text.trim().to_string();
            if text.is_empty() || is_low_quality_transcript_artifact(&text) {
                return None;
            }
            Some(MeetingTranscriptSegment {
                source: source.to_string(),
                speaker_id: speaker_id.to_string(),
                speaker_name: speaker_name.to_string(),
                start_ms: segment.start_ms,
                end_ms: segment.end_ms.max(segment.start_ms),
                text,
            })
        })
        .collect()
}

fn suppress_mic_echo_segments(
    segments: Vec<MeetingTranscriptSegment>,
    source_activity_path: Option<&Path>,
) -> Vec<MeetingTranscriptSegment> {
    let system_segments: Vec<&MeetingTranscriptSegment> = segments
        .iter()
        .filter(|segment| is_system_transcript_segment(segment))
        .collect();
    let mic_count = segments
        .iter()
        .filter(|segment| is_mic_transcript_segment(segment))
        .count();
    if mic_count == 0 || system_segments.is_empty() {
        return segments;
    }

    let activity_segments = load_source_activity_segments(source_activity_path);
    let has_activity = !activity_segments.is_empty();
    let mut best_matches = Vec::with_capacity(segments.len());
    let mut duplicate_count = 0usize;
    for segment in &segments {
        let echo_match = if is_mic_transcript_segment(segment) {
            best_system_echo_match(segment, &system_segments)
        } else {
            None
        };
        if echo_match.is_some_and(|matched| matched.similarity >= ECHO_DEDUPE_MIN_TEXT_SIMILARITY) {
            duplicate_count += 1;
        }
        best_matches.push(echo_match);
    }

    let duplicate_ratio = duplicate_count as f32 / mic_count as f32;
    let activity_summary = source_activity_summary(&activity_segments);
    let system_dominant_video_mode = has_activity
        && activity_summary.system_active_ms() >= ECHO_DEDUPE_VIDEO_MIN_SYSTEM_MS
        && activity_summary.local_ratio() <= ECHO_DEDUPE_VIDEO_MAX_LOCAL_RATIO
        && duplicate_ratio >= ECHO_DEDUPE_VIDEO_MIN_DUPLICATE_RATIO;

    let mut dropped = 0usize;
    let mut kept = Vec::with_capacity(segments.len());
    for (index, segment) in segments.into_iter().enumerate() {
        if !is_mic_transcript_segment(&segment) {
            kept.push(segment);
            continue;
        }

        let duration_ms = segment.end_ms.saturating_sub(segment.start_ms).max(1);
        let coverage = if has_activity {
            segment_activity_coverage(&segment, &activity_segments)
        } else {
            SegmentActivityCoverage::default()
        };
        let system_coverage = coverage.system_active_ratio(duration_ms);
        let local_coverage = coverage.local_mic_ratio(duration_ms);
        let silence_coverage = coverage.silence_ratio(duration_ms);
        let echo_match = best_matches[index];

        let strong_text_echo = echo_match.is_some_and(|matched| {
            matched.similarity >= ECHO_DEDUPE_STRONG_TEXT_SIMILARITY
                && (!has_activity
                    || coverage.covered_ms == 0
                    || (system_coverage >= 0.20 && local_coverage <= 0.60))
        });
        let source_confirmed_text_echo = echo_match.is_some_and(|matched| {
            matched.similarity >= ECHO_DEDUPE_MIN_TEXT_SIMILARITY
                && system_coverage >= ECHO_DEDUPE_MIN_SYSTEM_COVERAGE
                && local_coverage <= ECHO_DEDUPE_MAX_LOCAL_COVERAGE
        });
        let source_confirmed_video_bleed = system_dominant_video_mode
            && (system_coverage >= ECHO_DEDUPE_MIN_SYSTEM_COVERAGE
                || silence_coverage >= ECHO_DEDUPE_VIDEO_MIN_SILENCE_COVERAGE)
            && local_coverage <= ECHO_DEDUPE_MAX_LOCAL_COVERAGE;

        if strong_text_echo || source_confirmed_text_echo || source_confirmed_video_bleed {
            dropped += 1;
            continue;
        }
        kept.push(segment);
    }

    if dropped > 0 {
        tracing::info!(
            dropped,
            mic_count,
            duplicate_count,
            duplicate_ratio,
            system_dominant_video_mode,
            source_activity_path = source_activity_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_string()),
            "[meeting_engine] suppressed likely mic echo segments from dual-track meeting transcript"
        );
    }

    kept
}

fn load_source_activity_segments(path: Option<&Path>) -> Vec<SourceActivitySegment> {
    let Some(path) = path else {
        return Vec::new();
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "[meeting_engine] failed to read source activity json for echo suppression"
            );
            return Vec::new();
        }
    };

    match serde_json::from_slice::<MeetingAudioArtifact>(&bytes) {
        Ok(artifact) => artifact.source_activity_segments,
        Err(artifact_error) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => value
                .get("source_activity_segments")
                .cloned()
                .and_then(|segments| {
                    serde_json::from_value::<Vec<SourceActivitySegment>>(segments).ok()
                })
                .unwrap_or_else(|| {
                    tracing::warn!(
                        error = %artifact_error,
                        path = %path.display(),
                        "[meeting_engine] source activity json did not include readable segments"
                    );
                    Vec::new()
                }),
            Err(value_error) => {
                tracing::warn!(
                    error = %value_error,
                    artifact_error = %artifact_error,
                    path = %path.display(),
                    "[meeting_engine] failed to parse source activity json for echo suppression"
                );
                Vec::new()
            }
        },
    }
}

fn is_mic_transcript_segment(segment: &MeetingTranscriptSegment) -> bool {
    segment.source == "mic" || segment.speaker_id == "you"
}

fn is_system_transcript_segment(segment: &MeetingTranscriptSegment) -> bool {
    segment.source == "system" || segment.speaker_id == "speaker_1"
}

fn best_system_echo_match(
    mic: &MeetingTranscriptSegment,
    system_segments: &[&MeetingTranscriptSegment],
) -> Option<EchoMatch> {
    let mut best: Option<EchoMatch> = None;
    for system in system_segments {
        let start_gap_ms = mic.start_ms.abs_diff(system.start_ms);
        let interval_gap_ms =
            interval_gap_ms(mic.start_ms, mic.end_ms, system.start_ms, system.end_ms);
        if start_gap_ms > ECHO_DEDUPE_MAX_START_GAP_MS
            && interval_gap_ms > ECHO_DEDUPE_MAX_INTERVAL_GAP_MS
        {
            continue;
        }

        let similarity = transcript_token_similarity(&mic.text, &system.text);
        if similarity <= 0.0 {
            continue;
        }
        let candidate = EchoMatch {
            similarity,
            start_gap_ms,
            interval_gap_ms,
        };
        let replace = match best {
            None => true,
            Some(current) => {
                candidate.similarity > current.similarity
                    || (candidate.similarity == current.similarity
                        && candidate.start_gap_ms + candidate.interval_gap_ms
                            < current.start_gap_ms + current.interval_gap_ms)
            }
        };
        if replace {
            best = Some(candidate);
        }
    }
    best
}

fn transcript_token_similarity(left: &str, right: &str) -> f32 {
    let left_words = normalized_transcript_words(left);
    let right_words = normalized_transcript_words(right);
    if left_words.is_empty() || right_words.is_empty() {
        return 0.0;
    }
    if left_words == right_words {
        return 1.0;
    }

    let left_joined = left_words.join(" ");
    let right_joined = right_words.join(" ");
    if left_words.len().min(right_words.len()) >= 3
        && (left_joined.contains(&right_joined) || right_joined.contains(&left_joined))
    {
        return 0.92;
    }

    let left_set: std::collections::HashSet<&str> = left_words.iter().map(String::as_str).collect();
    let right_set: std::collections::HashSet<&str> =
        right_words.iter().map(String::as_str).collect();
    let intersection = left_set.intersection(&right_set).count();
    if intersection == 0 {
        return 0.0;
    }

    (2 * intersection) as f32 / (left_set.len() + right_set.len()) as f32
}

fn source_activity_summary(segments: &[SourceActivitySegment]) -> SourceActivitySummary {
    let mut summary = SourceActivitySummary::default();
    for segment in segments {
        let duration_ms = segment.end_ms.saturating_sub(segment.start_ms);
        match segment.source.as_str() {
            "local_mic" => summary.local_mic_ms += duration_ms,
            "system_audio" => summary.system_audio_ms += duration_ms,
            "overlap" => summary.overlap_ms += duration_ms,
            _ => {}
        }
    }
    summary
}

fn segment_activity_coverage(
    segment: &MeetingTranscriptSegment,
    activity_segments: &[SourceActivitySegment],
) -> SegmentActivityCoverage {
    let mut coverage = SegmentActivityCoverage::default();
    for activity in activity_segments {
        let overlap_start = segment.start_ms.max(activity.start_ms);
        let overlap_end = segment.end_ms.min(activity.end_ms);
        if overlap_end <= overlap_start {
            continue;
        }
        let duration_ms = overlap_end - overlap_start;
        coverage.covered_ms += duration_ms;
        match activity.source.as_str() {
            "local_mic" => coverage.local_mic_ms += duration_ms,
            "system_audio" => coverage.system_audio_ms += duration_ms,
            "overlap" => coverage.overlap_ms += duration_ms,
            "silence" => coverage.silence_ms += duration_ms,
            _ => {}
        }
    }
    coverage
}

fn interval_gap_ms(
    left_start_ms: u64,
    left_end_ms: u64,
    right_start_ms: u64,
    right_end_ms: u64,
) -> u64 {
    if left_end_ms < right_start_ms {
        right_start_ms - left_end_ms
    } else {
        left_start_ms.saturating_sub(right_end_ms)
    }
}

fn ratio_ms(numerator_ms: u64, denominator_ms: u64) -> f32 {
    if denominator_ms == 0 {
        0.0
    } else {
        numerator_ms as f32 / denominator_ms as f32
    }
}

fn filter_non_speech_transcript_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_low_quality_transcript_artifact(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_non_speech_transcript_artifact(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.chars().all(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                '-' | '_' | '.' | ',' | ';' | ':' | '!' | '?' | '*' | '"' | '\''
            )
    }) {
        return true;
    }

    if let Some(inner) = trimmed
        .strip_prefix('*')
        .and_then(|value| value.strip_suffix('*'))
    {
        let inner = inner.trim();
        if !inner.is_empty() && inner.chars().count() <= 80 {
            return true;
        }
    }

    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return false;
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return false;
    }

    let mut has_ascii_letter = false;
    for ch in inner.chars() {
        if ch.is_ascii_alphabetic() {
            has_ascii_letter = true;
            if !ch.is_ascii_uppercase() {
                return false;
            }
            continue;
        }
        if !(ch.is_ascii_digit() || matches!(ch, ' ' | '_' | '-' | '.' | '/')) {
            return false;
        }
    }

    has_ascii_letter
}

fn is_low_quality_transcript_artifact(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || is_non_speech_transcript_artifact(trimmed) {
        return true;
    }

    let words = normalized_transcript_words(trimmed);
    looks_like_repetitive_hallucination(&words)
}

fn normalized_transcript_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '\'' {
            current.push(ch);
            continue;
        }
        if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
}

fn looks_like_repetitive_hallucination(words: &[String]) -> bool {
    if words.len() < 8 {
        return false;
    }

    let unique: std::collections::HashSet<&str> = words.iter().map(String::as_str).collect();
    let unique_ratio = unique.len() as f32 / words.len() as f32;
    if unique_ratio < 0.28 {
        return true;
    }

    for ngram_size in 3..=8 {
        if words.len() < ngram_size * 3 {
            continue;
        }
        let mut counts: std::collections::HashMap<Vec<&str>, usize> =
            std::collections::HashMap::new();
        for window in words.windows(ngram_size) {
            *counts
                .entry(window.iter().map(String::as_str).collect())
                .or_insert(0) += 1;
        }
        if counts
            .values()
            .any(|count| *count >= 3 && (*count * ngram_size) as f32 >= words.len() as f32 * 0.45)
        {
            return true;
        }
    }

    false
}

fn source_sort_key(source: &str) -> u8 {
    match source {
        "you" => 0,
        "speaker_1" => 1,
        _ => 2,
    }
}

fn format_meeting_timeline_transcript(segments: &[MeetingTranscriptSegment]) -> String {
    segments
        .iter()
        .map(|segment| {
            format!(
                "[{} {}] {}",
                format_timestamp_ms(segment.start_ms),
                segment.speaker_name,
                segment.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_timestamp_ms(ms: u64) -> String {
    let total_seconds = ms / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn transcribe_with_whisper_cpp(
    summary: &MicCaptureSummary,
    paths: &TranscriptPaths,
    config: &WhisperCppConfig,
    track: MeetingAudioTrack,
    cancel_check: Option<&dyn Fn() -> bool>,
    progress: Option<&dyn Fn(WhisperChunkProgress)>,
) -> Result<WhisperTranscriptionDone, String> {
    let chunk_ms = final_asr_chunk_ms();
    if summary.duration_ms > chunk_ms {
        return transcribe_with_whisper_cpp_chunked(
            summary,
            paths,
            config,
            track,
            chunk_ms,
            cancel_check,
            progress,
        );
    }
    if let Some(progress) = progress {
        progress(WhisperChunkProgress {
            track,
            current: 1,
            total: 1,
        });
    }
    transcribe_with_whisper_cpp_for(summary, paths, config, track, WHISPER_TIMEOUT, cancel_check)
}

fn whisper_process_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(windows)]
fn hide_meeting_child_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_meeting_child_console(_command: &mut Command) {}

fn fail_if_cancelled(cancel_check: Option<&dyn Fn() -> bool>, label: &str) -> Result<(), String> {
    if cancel_check.is_some_and(|check| check()) {
        return Err(format!("{label} cancelled"));
    }
    Ok(())
}

fn is_cancelled_subprocess_error(message: &str) -> bool {
    message.to_ascii_lowercase().contains("cancelled")
}

fn acquire_whisper_process_lock(
    cancel_check: Option<&dyn Fn() -> bool>,
) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    loop {
        fail_if_cancelled(cancel_check, "whisper.cpp")?;
        match whisper_process_lock().try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("whisper.cpp process lock poisoned".to_string());
            }
        }
    }
}

fn final_asr_chunk_ms() -> u64 {
    env_u64(
        "AIRNOTE_MEETING_FINAL_ASR_CHUNK_SECS",
        DEFAULT_FINAL_ASR_CHUNK_SECS,
    )
    .clamp(60, 900)
    .saturating_mul(1_000)
}

fn final_asr_chunk_count(summary: &MicCaptureSummary, chunk_ms: u64) -> u64 {
    if summary.samples_written == 0 {
        return 0;
    }
    let chunk_samples = chunk_ms
        .max(1)
        .saturating_mul(SAMPLE_RATE as u64)
        .saturating_div(1_000)
        .max(1);
    summary
        .samples_written
        .saturating_add(chunk_samples - 1)
        .saturating_div(chunk_samples)
        .max(1)
}

fn may_have_final_asr_work(summary: &MicCaptureSummary) -> bool {
    summary.samples_written > 0 && summary.peak >= ASR_MIN_PEAK_FOR_TRANSCRIPTION
}

fn transcribe_with_whisper_cpp_chunked(
    summary: &MicCaptureSummary,
    paths: &TranscriptPaths,
    config: &WhisperCppConfig,
    track: MeetingAudioTrack,
    chunk_ms: u64,
    cancel_check: Option<&dyn Fn() -> bool>,
    progress: Option<&dyn Fn(WhisperChunkProgress)>,
) -> Result<WhisperTranscriptionDone, String> {
    let started = Instant::now();
    let chunks = write_wav_asr_chunks(summary, chunk_ms)?;
    if chunks.is_empty() {
        return Err("no audio samples found for whisper.cpp transcription".to_string());
    }
    if chunks.len() <= 1 {
        if let Some(progress) = progress {
            progress(WhisperChunkProgress {
                track,
                current: 1,
                total: 1,
            });
        }
        return transcribe_with_whisper_cpp_for(
            summary,
            paths,
            config,
            track,
            WHISPER_TIMEOUT,
            cancel_check,
        );
    }

    tracing::info!(
        path = %summary.path.display(),
        chunks = chunks.len(),
        chunk_ms,
        model = %config.model.display(),
        "[meeting_engine] transcribing long meeting audio in final ASR chunks"
    );

    let mut segments = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        fail_if_cancelled(cancel_check, "whisper.cpp")?;
        if let Some(progress) = progress {
            progress(WhisperChunkProgress {
                track,
                current: (index + 1) as u64,
                total: chunks.len() as u64,
            });
        }
        if !has_transcribable_audio(&chunk.summary) {
            tracing::info!(
                chunk = index + 1,
                total = chunks.len(),
                start_ms = chunk.start_ms,
                peak = chunk.summary.peak,
                "[meeting_engine] skipping silent final ASR chunk"
            );
            continue;
        }

        let chunk_dir = chunk
            .summary
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let stem = chunk
            .summary
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .unwrap_or("chunk");
        let chunk_paths = transcript_paths_for_stem(chunk_dir, stem);
        if let Some(done) = read_cached_whisper_transcription(&chunk.summary, &chunk_paths, config)
        {
            segments.extend(done.segments.into_iter().filter_map(|mut segment| {
                segment.start_ms = chunk.start_ms.saturating_add(segment.start_ms);
                segment.end_ms = chunk.start_ms.saturating_add(segment.end_ms);
                segment.start_ms = segment.start_ms.min(summary.duration_ms);
                segment.end_ms = segment
                    .end_ms
                    .max(segment.start_ms)
                    .min(summary.duration_ms);
                (!segment.text.trim().is_empty()).then_some(segment)
            }));
            tracing::info!(
                chunk = index + 1,
                total = chunks.len(),
                start_ms = chunk.start_ms,
                "[meeting_engine] reused cached final ASR chunk transcript"
            );
            continue;
        }
        match transcribe_with_whisper_cpp_for(
            &chunk.summary,
            &chunk_paths,
            config,
            track,
            WHISPER_TIMEOUT,
            cancel_check,
        ) {
            Ok(done) => {
                segments.extend(done.segments.into_iter().filter_map(|mut segment| {
                    segment.start_ms = chunk.start_ms.saturating_add(segment.start_ms);
                    segment.end_ms = chunk.start_ms.saturating_add(segment.end_ms);
                    segment.start_ms = segment.start_ms.min(summary.duration_ms);
                    segment.end_ms = segment
                        .end_ms
                        .max(segment.start_ms)
                        .min(summary.duration_ms);
                    (!segment.text.trim().is_empty()).then_some(segment)
                }));
                tracing::info!(
                    chunk = index + 1,
                    total = chunks.len(),
                    start_ms = chunk.start_ms,
                    "[meeting_engine] final ASR chunk completed"
                );
            }
            Err(e) if is_empty_whisper_chunk_error(&e) => {
                tracing::info!(
                    chunk = index + 1,
                    total = chunks.len(),
                    start_ms = chunk.start_ms,
                    error = %e,
                    "[meeting_engine] final ASR chunk had no confident speech"
                );
            }
            Err(e) => {
                return Err(format!(
                    "whisper.cpp chunk {}/{} at {} failed: {}",
                    index + 1,
                    chunks.len(),
                    format_timestamp_ms(chunk.start_ms),
                    e
                ));
            }
        }
    }

    segments.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| left.end_ms.cmp(&right.end_ms))
    });
    let segments = suppress_repeated_whisper_segment_runs(segments);
    let transcript = segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if transcript.is_empty() {
        return Err("whisper.cpp returned no confident speech transcript".to_string());
    }

    write_atomic(&paths.text, transcript.as_bytes())
        .map_err(|e| format!("failed to write transcript text: {e}"))?;
    if let Some(chunk_dir) = chunks
        .first()
        .and_then(|chunk| chunk.summary.path.parent())
        .map(Path::to_path_buf)
    {
        let _ = fs::remove_dir_all(chunk_dir);
    }

    Ok(WhisperTranscriptionDone {
        transcript,
        latency_ms: started.elapsed().as_millis() as u64,
        segments,
    })
}

fn is_empty_whisper_chunk_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("no confident speech transcript") || message.contains("below speech threshold")
}

fn write_wav_asr_chunks(
    summary: &MicCaptureSummary,
    chunk_ms: u64,
) -> Result<Vec<WhisperAudioChunk>, String> {
    repair_wav_header_sizes(&summary.path)?;
    let mut reader = hound::WavReader::open(&summary.path)
        .map_err(|e| format!("failed to open WAV for final ASR chunking: {e}"))?;
    validate_merge_wav_spec("final ASR input", reader.spec())?;

    let parent = summary.path.parent().unwrap_or_else(|| Path::new("."));
    let stem = summary
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("audio");
    let chunk_dir = parent.join(format!("{stem}.asr-chunks"));
    // Keep prior chunk transcripts in place. If the user cancels a long
    // transcription, retry can resume from the completed chunks instead of
    // throwing away several minutes of local ASR work.
    fs::create_dir_all(&chunk_dir)
        .map_err(|e| format!("failed to create final ASR chunk directory: {e}"))?;

    let chunk_samples = chunk_ms
        .max(1)
        .saturating_mul(SAMPLE_RATE as u64)
        .saturating_div(1_000)
        .max(1);
    let mut chunks = Vec::new();
    let mut writer: Option<hound::WavWriter<BufWriter<File>>> = None;
    let mut chunk_path: Option<PathBuf> = None;
    let mut chunk_index = 0usize;
    let mut chunk_start_sample = 0_u64;
    let mut samples_in_chunk = 0_u64;
    let mut total_samples = 0_u64;
    let mut peak_i16 = 0_i16;

    for sample in reader.samples::<i16>() {
        if writer.is_none() {
            let path = chunk_dir.join(format!("chunk-{chunk_index:05}.wav"));
            writer = Some(create_audio_wav_writer(&path, "final ASR chunk")?);
            chunk_path = Some(path);
            chunk_start_sample = total_samples;
            samples_in_chunk = 0;
            peak_i16 = 0;
        }

        let sample = sample.map_err(|e| format!("failed to read final ASR input sample: {e}"))?;
        writer
            .as_mut()
            .ok_or_else(|| "final ASR chunk writer was not initialized".to_string())?
            .write_sample(sample)
            .map_err(|e| format!("failed to write final ASR chunk sample: {e}"))?;
        samples_in_chunk = samples_in_chunk.saturating_add(1);
        total_samples = total_samples.saturating_add(1);
        peak_i16 = peak_i16.max(sample.saturating_abs());

        if samples_in_chunk >= chunk_samples {
            let finished_writer = writer
                .take()
                .ok_or_else(|| "final ASR chunk writer was not initialized".to_string())?;
            let finished_path = chunk_path
                .take()
                .ok_or_else(|| "final ASR chunk path was not initialized".to_string())?;
            chunks.push(finish_wav_asr_chunk(
                finished_writer,
                finished_path,
                summary,
                chunk_start_sample,
                samples_in_chunk,
                peak_i16,
            )?);
            chunk_index = chunk_index.saturating_add(1);
        }
    }

    if let Some(finished_writer) = writer {
        let finished_path =
            chunk_path.ok_or_else(|| "final ASR chunk path was not initialized".to_string())?;
        chunks.push(finish_wav_asr_chunk(
            finished_writer,
            finished_path,
            summary,
            chunk_start_sample,
            samples_in_chunk,
            peak_i16,
        )?);
    }

    Ok(chunks)
}

fn finish_wav_asr_chunk(
    writer: hound::WavWriter<BufWriter<File>>,
    path: PathBuf,
    source_summary: &MicCaptureSummary,
    start_sample: u64,
    samples_written: u64,
    peak_i16: i16,
) -> Result<WhisperAudioChunk, String> {
    writer
        .finalize()
        .map_err(|e| format!("failed to finalize final ASR chunk WAV: {e}"))?;
    repair_wav_header_sizes(&path)?;

    Ok(WhisperAudioChunk {
        summary: MicCaptureSummary {
            path,
            samples_written,
            dropped_chunks: source_summary.dropped_chunks,
            native_rate: source_summary.native_rate,
            duration_ms: samples_written.saturating_mul(1_000) / SAMPLE_RATE as u64,
            peak: peak_i16 as f32 / i16::MAX as f32,
        },
        start_ms: start_sample.saturating_mul(1_000) / SAMPLE_RATE as u64,
    })
}

fn read_cached_whisper_transcription(
    summary: &MicCaptureSummary,
    paths: &TranscriptPaths,
    config: &WhisperCppConfig,
) -> Option<WhisperTranscriptionDone> {
    if !paths.text.is_file() {
        return None;
    }
    let bytes = fs::read(&paths.text).ok()?;
    let file_transcript = String::from_utf8_lossy(&bytes).trim().to_string();
    if file_transcript.is_empty() || is_low_quality_transcript_artifact(&file_transcript) {
        return None;
    }
    let segments = whisper_segments_from_json(paths, summary.duration_ms, config)
        .unwrap_or_else(|| fallback_whisper_segments(summary.duration_ms, &file_transcript));
    let transcript = segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if transcript.trim().is_empty() || is_low_quality_transcript_artifact(&transcript) {
        return None;
    }
    Some(WhisperTranscriptionDone {
        transcript,
        latency_ms: 0,
        segments,
    })
}

fn transcribe_with_whisper_cpp_for(
    summary: &MicCaptureSummary,
    paths: &TranscriptPaths,
    config: &WhisperCppConfig,
    track: MeetingAudioTrack,
    timeout: Duration,
    cancel_check: Option<&dyn Fn() -> bool>,
) -> Result<WhisperTranscriptionDone, String> {
    fail_if_cancelled(cancel_check, "whisper.cpp")?;
    repair_wav_header_sizes(&summary.path)?;
    let whisper_audio_path = prepare_whisper_audio_input(summary)?;
    let language = whisper_language_for_track(track, &config.language);

    let mut cmd = Command::new(&config.binary);
    cmd.arg("-m")
        .arg(&config.model)
        .arg("-f")
        .arg(&whisper_audio_path)
        .arg("-l")
        .arg(&language)
        .arg("-mc")
        .arg(config.max_context_tokens.to_string())
        .arg("-otxt")
        .arg("-ojf")
        .arg("-of")
        .arg(&paths.whisper_out_base)
        .arg("-np")
        // Discard whisper's stdout/stderr instead of piping it. The transcript is
        // read from the -otxt/-ojf files (whisper_out_base), NOT stdout. Piping but
        // only draining at exit (wait_with_output) let whisper.cpp's stderr
        // (model-load/system_info, printed even with -np) fill the ~64KB OS pipe
        // buffer on long transcripts → whisper blocks on write → the job stalls
        // until WHISPER_TIMEOUT. Fires per ~30s live window too. null = no buffer.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if config.suppress_non_speech {
        cmd.arg("-sns");
    }
    if config.no_fallback {
        cmd.arg("-nf");
    }
    if let Some(threshold) = config.no_speech_threshold {
        cmd.arg("-nth").arg(threshold.to_string());
    }
    if let Some(threshold) = config.logprob_threshold {
        cmd.arg("-lpt").arg(threshold.to_string());
    }
    if let Some(threshold) = config.entropy_threshold {
        cmd.arg("-et").arg(threshold.to_string());
    }
    if whisper_translate_for_track(track) {
        cmd.arg("-tr");
    }
    if let Some(prompt) = &config.prompt {
        if !prompt.trim().is_empty() {
            cmd.arg("--prompt").arg(prompt);
        }
    }
    // Silero VAD: only feed detected speech to whisper (kills silence/bleed
    // hallucinations). Applies to both live windows and the final transcript.
    if let Some(vad_model) = &config.vad_model {
        cmd.arg("--vad")
            .arg("-vm")
            .arg(vad_model)
            .arg("-vt")
            .arg(config.vad_threshold.to_string())
            .arg("-vp")
            .arg(config.vad_speech_pad_ms.to_string())
            .arg("-vsd")
            .arg(config.vad_min_silence_ms.to_string());
    }

    // Re-validate just before spawn: the model or binary can disappear between
    // config resolution (enqueue) and execution (a user deletes a model, disk
    // cleanup, a broken symlink). Fail with a clear, terminal message instead of
    // letting whisper-cli emit a cryptic error that retries forever.
    if !is_usable_whisper_model(&config.model) {
        return Err(format!(
            "whisper model file is missing or corrupt: {} — reinstall it from Settings → Meeting",
            config.model.display()
        ));
    }
    if !config.binary.is_file() {
        return Err(format!(
            "whisper.cpp binary not found at {} — the transcription engine is missing from this build",
            config.binary.display()
        ));
    }

    // Large whisper.cpp models can use ~2 GB RSS each. Serializing every local
    // whisper-cli launch prevents live chunks and final/re-transcribe jobs from
    // briefly loading multiple copies of the model on light laptops.
    let _whisper_process_guard = acquire_whisper_process_lock(cancel_check)?;
    fail_if_cancelled(cancel_check, "whisper.cpp")?;
    let started = Instant::now();
    hide_meeting_child_console(&mut cmd);
    #[cfg(unix)]
    {
        // whisper-cli can be long-running and may spawn helper work internally;
        // isolate it so timeout cleanup can kill the whole process group.
        cmd.process_group(0);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn whisper.cpp: {e}"))?;
    let output = wait_with_timeout(child, timeout, cancel_check)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        // No output on a non-zero exit almost always means a crash (OOM / SIGSEGV
        // in ggml, or a corrupt model) — give an actionable hint.
        if detail.is_empty() {
            return Err(format!(
                "whisper.cpp crashed ({}, no output) — likely out of memory or a corrupt model; try a smaller model",
                output.status
            ));
        }
        return Err(format!(
            "whisper.cpp exited with {}: {}",
            output.status,
            truncate_error(detail)
        ));
    }

    let file_transcript = if paths.whisper_txt.is_file() {
        // Lossy read: tolerate the occasional invalid UTF-8 byte whisper.cpp
        // emits in long Devanagari output instead of failing the whole job.
        let bytes = fs::read(&paths.whisper_txt)
            .map_err(|e| format!("failed to read whisper output: {e}"))?;
        String::from_utf8_lossy(&bytes).trim().to_string()
    } else {
        clean_whisper_stdout(&String::from_utf8_lossy(&output.stdout))
    };
    let json_segments = whisper_segments_from_json(paths, summary.duration_ms, config);
    let (transcript, segments) = if let Some(segments) = json_segments {
        let transcript = segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        (transcript, segments)
    } else {
        let transcript = filter_non_speech_transcript_lines(&file_transcript);
        let segments = fallback_whisper_segments(summary.duration_ms, &transcript);
        (transcript, segments)
    };
    if transcript.is_empty() {
        return Err("whisper.cpp returned no confident speech transcript".to_string());
    }

    // NOTE: romanization is intentionally NOT done here. Live windows stay in
    // whisper's natural script (Devanagari/English) — easiest for the model and
    // lowest latency. The final transcript is romanized to Hinglish downstream
    // (final segments + LLM cleanup), so live captions and the saved transcript
    // can differ in script by design.
    let _ = config.romanize;

    write_atomic(&paths.text, transcript.as_bytes())
        .map_err(|e| format!("failed to write transcript text: {e}"))?;

    Ok(WhisperTranscriptionDone {
        transcript,
        latency_ms,
        segments,
    })
}

#[derive(Debug, Deserialize)]
struct WhisperJsonOutput {
    transcription: Vec<WhisperJsonSegment>,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonSegment {
    offsets: Option<WhisperJsonOffsets>,
    text: String,
    tokens: Option<Vec<WhisperJsonToken>>,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonOffsets {
    from: u64,
    to: u64,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonToken {
    text: String,
    p: Option<f64>,
}

fn whisper_segments_from_json(
    paths: &TranscriptPaths,
    duration_ms: u64,
    config: &WhisperCppConfig,
) -> Option<Vec<RawTranscriptSegment>> {
    if !paths.whisper_json.is_file() {
        return None;
    }

    // Read lossily: whisper.cpp occasionally emits an invalid UTF-8 byte in long
    // Devanagari/non-Latin output, which would make strict reads fail entirely.
    let output = match fs::read(&paths.whisper_json)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .and_then(|json| serde_json::from_str::<WhisperJsonOutput>(&json).ok())
    {
        Some(output) => output,
        None => {
            tracing::warn!(
                path = %paths.whisper_json.display(),
                "[meeting_engine] whisper JSON missing or invalid; using whole transcript segment"
            );
            return None;
        }
    };

    let mut rejected = 0usize;
    let mut candidates: Vec<WhisperSegmentCandidate> = output
        .transcription
        .into_iter()
        .filter_map(|segment| {
            let text = filter_non_speech_transcript_lines(&segment.text);
            if text.is_empty() {
                rejected = rejected.saturating_add(1);
                return None;
            }
            let confidence = whisper_segment_confidence(&segment);
            let Some(offsets) = segment.offsets else {
                rejected = rejected.saturating_add(1);
                return None;
            };
            Some(WhisperSegmentCandidate {
                segment: RawTranscriptSegment {
                    start_ms: offsets.from.min(duration_ms),
                    end_ms: offsets.to.max(offsets.from).min(duration_ms),
                    text,
                },
                confidence,
                repeated_run: false,
            })
        })
        .collect();
    let repetition_rejected = mark_repeated_whisper_candidate_runs(&mut candidates);
    let segments: Vec<RawTranscriptSegment> = candidates
        .into_iter()
        .filter_map(|candidate| {
            if candidate.repeated_run
                || !is_usable_whisper_segment(
                    &candidate.segment.text,
                    candidate.confidence,
                    config.min_segment_confidence,
                )
            {
                rejected = rejected.saturating_add(1);
                return None;
            }
            Some(candidate.segment)
        })
        .collect();
    let accepted_before_repetition_gate = segments.len();
    let segments = suppress_repeated_whisper_segment_runs(segments);
    let accepted_repetition_rejected =
        accepted_before_repetition_gate.saturating_sub(segments.len());

    if rejected > 0 || repetition_rejected > 0 || accepted_repetition_rejected > 0 {
        tracing::info!(
            path = %paths.whisper_json.display(),
            accepted = segments.len(),
            rejected,
            repetition_rejected,
            accepted_repetition_rejected,
            "[meeting_engine] whisper quality gate filtered segments"
        );
    }

    Some(segments)
}

fn fallback_whisper_segments(duration_ms: u64, transcript: &str) -> Vec<RawTranscriptSegment> {
    if transcript.trim().is_empty() {
        Vec::new()
    } else {
        vec![RawTranscriptSegment {
            start_ms: 0,
            end_ms: duration_ms,
            text: transcript.trim().to_string(),
        }]
    }
}

fn whisper_segment_confidence(segment: &WhisperJsonSegment) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0usize;
    for token in segment.tokens.as_deref().unwrap_or_default() {
        let text = token.text.trim();
        if text.is_empty() || (text.starts_with("[_") && text.ends_with("_]")) {
            continue;
        }
        let Some(probability) = token.p else {
            continue;
        };
        if !(0.0..=1.0).contains(&probability) || !probability.is_finite() {
            continue;
        }
        sum += probability;
        count += 1;
    }

    (count > 0).then(|| sum / count as f64)
}

fn is_usable_whisper_segment(
    text: &str,
    confidence: Option<f64>,
    min_confidence: Option<f64>,
) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || is_low_quality_transcript_artifact(trimmed) {
        return false;
    }

    let words = normalized_transcript_words(trimmed);
    if words.len() <= 2 && confidence.is_some_and(|value| value < 0.70) {
        return false;
    }

    match (confidence, min_confidence) {
        (Some(confidence), Some(min_confidence)) => confidence >= min_confidence,
        _ => true,
    }
}

fn suppress_repeated_whisper_segment_runs(
    segments: Vec<RawTranscriptSegment>,
) -> Vec<RawTranscriptSegment> {
    let mut filtered = Vec::with_capacity(segments.len());
    let mut index = 0usize;

    while index < segments.len() {
        let mut end = index + 1;
        while end < segments.len()
            && transcript_segments_are_near_duplicates(&segments[index].text, &segments[end].text)
        {
            end += 1;
        }

        if end - index >= 3 {
            index = end;
            continue;
        }

        filtered.extend_from_slice(&segments[index..end]);
        index = end;
    }

    filtered
}

fn mark_repeated_whisper_candidate_runs(candidates: &mut [WhisperSegmentCandidate]) -> usize {
    let mut marked = 0usize;
    let mut index = 0usize;

    while index < candidates.len() {
        let mut end = index + 1;
        while end < candidates.len()
            && transcript_segments_are_near_duplicates(
                &candidates[index].segment.text,
                &candidates[end].segment.text,
            )
        {
            end += 1;
        }

        if end - index >= 3 {
            for candidate in &mut candidates[index..end] {
                if !candidate.repeated_run {
                    candidate.repeated_run = true;
                    marked += 1;
                }
            }
        }

        index = end;
    }

    marked
}

fn transcript_segments_are_near_duplicates(left: &str, right: &str) -> bool {
    let left_words = normalized_transcript_words(left);
    let right_words = normalized_transcript_words(right);
    if left_words.len().min(right_words.len()) < 4 {
        return false;
    }

    let left_set: std::collections::HashSet<&str> = left_words.iter().map(String::as_str).collect();
    let right_set: std::collections::HashSet<&str> =
        right_words.iter().map(String::as_str).collect();
    let intersection = left_set.intersection(&right_set).count() as f32;
    let union = left_set.union(&right_set).count() as f32;
    if union <= 0.0 {
        return false;
    }

    let jaccard = intersection / union;
    let containment = intersection / left_set.len().min(right_set.len()) as f32;
    jaccard >= 0.78 || containment >= 0.85
}

fn wait_with_timeout(
    child: std::process::Child,
    timeout: Duration,
    cancel_check: Option<&dyn Fn() -> bool>,
) -> Result<Output, String> {
    wait_with_timeout_for_cancel(child, timeout, "whisper.cpp", cancel_check)
}

fn wait_with_timeout_for(
    child: std::process::Child,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    wait_with_timeout_for_cancel(child, timeout, label, None)
}

fn wait_with_timeout_for_cancel(
    mut child: std::process::Child,
    timeout: Duration,
    label: &str,
    cancel_check: Option<&dyn Fn() -> bool>,
) -> Result<Output, String> {
    let started = Instant::now();
    let watchdog_done = spawn_timeout_watchdog(child.id(), timeout, label.to_string());
    loop {
        if cancel_check.is_some_and(|check| check()) {
            terminate_cancelled_child(&mut child, label);
            watchdog_done.store(true, Ordering::SeqCst);
            return Err(format!("{label} cancelled"));
        }
        match child.try_wait() {
            Ok(Some(_status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("failed to collect {label} output: {e}"));
                watchdog_done.store(true, Ordering::SeqCst);
                return output;
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    terminate_timed_out_child(&mut child, label);
                    watchdog_done.store(true, Ordering::SeqCst);
                    return Err(format!("{label} timed out after {}s", timeout.as_secs()));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                watchdog_done.store(true, Ordering::SeqCst);
                return Err(format!("failed to poll {label}: {e}"));
            }
        }
    }
}

fn spawn_timeout_watchdog(pid: u32, timeout: Duration, label: String) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    {
        let done_for_thread = Arc::clone(&done);
        thread::spawn(move || {
            let watchdog_delay = timeout.saturating_add(Duration::from_secs(1));
            thread::sleep(watchdog_delay);
            if done_for_thread.load(Ordering::SeqCst) {
                return;
            }
            tracing::warn!(
                pid,
                label = %label,
                timeout_secs = timeout.as_secs(),
                "[meeting_engine] watchdog killing timed-out subprocess group"
            );
            terminate_process_group(pid);
        });
    }

    // Windows backstop: the in-loop terminate_timed_out_child only fires if the
    // poll loop keeps making progress. If the loop ever wedges, a runaway
    // whisper-cli.exe (~2GB RSS) would never be reaped. This independent thread
    // kills the process tree after timeout + grace regardless of the loop.
    #[cfg(windows)]
    {
        let done_for_thread = Arc::clone(&done);
        thread::spawn(move || {
            let watchdog_delay = timeout.saturating_add(Duration::from_secs(2));
            thread::sleep(watchdog_delay);
            if done_for_thread.load(Ordering::SeqCst) {
                return;
            }
            tracing::warn!(
                pid,
                label = %label,
                timeout_secs = timeout.as_secs(),
                "[meeting_engine] watchdog killing timed-out subprocess tree (windows)"
            );
            terminate_windows_process_tree(pid, true);
        });
    }

    done
}

fn terminate_timed_out_child(child: &mut std::process::Child, label: &str) {
    terminate_child_process(child, label, "timed out");
}

fn terminate_cancelled_child(child: &mut std::process::Child, label: &str) {
    terminate_child_process(child, label, "cancelled");
}

fn terminate_child_process(child: &mut std::process::Child, label: &str, reason: &str) {
    let pid = child.id();
    tracing::warn!(
        pid,
        label,
        reason,
        "[meeting_engine] terminating subprocess"
    );

    #[cfg(unix)]
    {
        terminate_process_group(pid);
        thread::sleep(Duration::from_millis(250));
        if matches!(child.try_wait(), Ok(None)) {
            kill_process_group(pid, libc::SIGKILL);
        }
    }

    #[cfg(windows)]
    {
        terminate_windows_process_tree(pid, false);
        thread::sleep(Duration::from_millis(250));
        if matches!(child.try_wait(), Ok(None)) {
            terminate_windows_process_tree(pid, true);
        }
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = child.kill();
    }

    let _ = child.wait();
}

#[cfg(windows)]
fn terminate_windows_process_tree(pid: u32, force: bool) {
    let mut cmd = Command::new("taskkill");
    cmd.args(windows_taskkill_args(pid, force))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Suppress the brief console window taskkill would otherwise flash.
    hide_meeting_child_console(&mut cmd);
    let _ = cmd.status();
}

#[cfg(any(windows, test))]
fn windows_taskkill_args(pid: u32, force: bool) -> Vec<String> {
    let mut args = vec!["/PID".to_string(), pid.to_string(), "/T".to_string()];
    if force {
        args.push("/F".to_string());
    }
    args
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    kill_process_group(pid, libc::SIGTERM);
    thread::sleep(Duration::from_millis(250));
    kill_process_group(pid, libc::SIGKILL);
}

#[cfg(unix)]
fn kill_process_group(pid: u32, signal: libc::c_int) {
    let pid = pid as libc::pid_t;
    unsafe {
        let _ = libc::kill(-pid, signal);
        let _ = libc::kill(pid, signal);
    }
}

fn run_final_diarization_stage(
    transcription_state: &Arc<Mutex<TranscriptionSnapshot>>,
    transcript_paths: &TranscriptPaths,
    audio_path: &Path,
) -> MeetingFinalDiarizationSnapshot {
    let Some(paths) = final_diarization_paths_for_transcript(transcript_paths) else {
        return MeetingFinalDiarizationSnapshot::skipped(
            "skipped_no_output_path",
            "could not derive final diarization output paths",
            None,
        );
    };

    if !meeting_final_diarization_enabled() {
        return MeetingFinalDiarizationSnapshot::skipped(
            "skipped_disabled",
            "final diarization disabled; optimized meeting flow uses source labels only",
            Some(paths),
        );
    }

    let runner = match meeting_final_diarization_runner() {
        Ok(Some(runner)) => runner,
        Ok(None) => {
            return MeetingFinalDiarizationSnapshot::skipped(
                "skipped_missing_command",
                "speaker detection helper is not configured",
                Some(paths),
            );
        }
        Err(e) => {
            return MeetingFinalDiarizationSnapshot::skipped(
                "skipped_invalid_config",
                e,
                Some(paths),
            );
        }
    };

    if matches!(runner, MeetingFinalDiarizationRunner::LightOnnx) {
        if let Some(message) = light_diarization_skip_reason(audio_path) {
            tracing::warn!(
                audio = %audio_path.display(),
                reason = %message,
                "[meeting_engine] skipping light final diarization"
            );
            return MeetingFinalDiarizationSnapshot::skipped(
                "skipped_long_audio",
                message,
                Some(paths),
            );
        }
    }

    {
        let mut transcription = transcription_state.lock_recover();
        transcription.status = "final_diarizing".to_string();
        transcription.progress = None;
        transcription.final_diarization =
            MeetingFinalDiarizationSnapshot::running(runner.provider(), &paths);
    }

    let started = Instant::now();
    let provider = runner.provider();
    let result = match &runner {
        MeetingFinalDiarizationRunner::LightOnnx => {
            run_light_final_diarization(audio_path, transcript_paths, &paths)
        }
        MeetingFinalDiarizationRunner::Command(config) => {
            run_final_diarization_command(config, audio_path, transcript_paths, &paths)
        }
    };
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(()) => MeetingFinalDiarizationSnapshot::completed(provider, latency_ms, &paths),
        Err(e) => {
            write_final_diarization_failure(&paths.diarization_json, &provider, &e);
            MeetingFinalDiarizationSnapshot::failed(provider, latency_ms, &paths, e)
        }
    }
}

fn light_diarization_skip_reason(audio_path: &Path) -> Option<String> {
    let duration_ms = wav_duration_ms(audio_path)?;
    light_diarization_skip_reason_for_duration(duration_ms, light_diarization_max_audio_ms())
}

fn light_diarization_skip_reason_for_duration(duration_ms: u64, max_ms: u64) -> Option<String> {
    if duration_ms <= max_ms {
        return None;
    }
    Some(format!(
        "light speaker detection skipped for {:.1}s audio; limit is {:.1}s on this device",
        duration_ms as f64 / 1000.0,
        max_ms as f64 / 1000.0
    ))
}

fn light_diarization_max_audio_ms() -> u64 {
    env_u64(
        "AIRNOTE_MEETING_LIGHT_DIARIZATION_MAX_AUDIO_SECS",
        DEFAULT_LIGHT_DIARIZATION_MAX_AUDIO_SECS,
    )
    .saturating_mul(1000)
}

fn load_final_transcript_text(
    final_diarization: &MeetingFinalDiarizationSnapshot,
) -> Result<Option<String>, String> {
    if final_diarization.status != "completed" {
        return Ok(None);
    }
    let Some(path) = &final_diarization.transcript_json_path else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }

    let json = fs::read_to_string(path)
        .map_err(|e| format!("failed to read final transcript json: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| format!("failed to parse final transcript json: {e}"))?;
    let transcript = parsed
        .get("transcript")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if transcript.is_some() {
        return Ok(transcript);
    }

    let text_path = path.with_extension("txt");
    if !text_path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&text_path)
        .map_err(|e| format!("failed to read final transcript text: {e}"))?;
    Ok(Some(text.trim().to_string()).filter(|value| !value.is_empty()))
}

fn run_final_diarization_command(
    config: &MeetingFinalDiarizationConfig,
    audio_path: &Path,
    transcript_paths: &TranscriptPaths,
    final_paths: &FinalDiarizationPaths,
) -> Result<(), String> {
    let mut cmd = Command::new(&config.command);
    if let Some(script) = &config.script {
        cmd.arg(script);
    }
    cmd.arg("--audio")
        .arg(audio_path)
        .arg("--transcript-json")
        .arg(&transcript_paths.json)
        .arg("--diarization-out")
        .arg(&final_paths.diarization_json)
        .arg("--transcript-out")
        .arg(&final_paths.transcript_json)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    hide_meeting_child_console(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn final diarization command: {e}"))?;
    let output = wait_with_timeout_for(child, config.timeout, "meeting final diarization")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "final diarization exited with {}: {}",
            output.status,
            truncate_error(if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            })
        ));
    }
    if !final_paths.diarization_json.is_file() {
        return Err(format!(
            "final diarization did not write {}",
            final_paths.diarization_json.display()
        ));
    }
    if !final_paths.transcript_json.is_file() {
        return Err(format!(
            "final diarization did not write {}",
            final_paths.transcript_json.display()
        ));
    }
    Ok(())
}

fn run_light_final_diarization(
    _audio_path: &Path,
    _transcript_paths: &TranscriptPaths,
    _final_paths: &FinalDiarizationPaths,
) -> Result<(), String> {
    Err("final diarization has been removed from the meeting pipeline".to_string())
}

fn read_meeting_transcript_artifact(path: &Path) -> Result<MeetingTranscriptArtifact, String> {
    let json = fs::read_to_string(path)
        .map_err(|e| format!("failed to read transcript artifact {}: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| {
        format!(
            "failed to parse transcript artifact {}: {e}",
            path.display()
        )
    })
}

fn seconds_to_ms(seconds: f32) -> u64 {
    (seconds.max(0.0) * 1_000.0).round() as u64
}

fn assign_light_diarization_to_transcript(
    transcript_segments: &[MeetingTranscriptSegment],
    turns: &[LightDiarizationTurn],
) -> Vec<MeetingTranscriptSegment> {
    let mut speaker_order = std::collections::HashMap::<String, usize>::new();
    transcript_segments
        .iter()
        .map(|segment| {
            if segment.source == "mic" || segment.speaker_id == "you" {
                return segment.clone();
            }
            let mut next = segment.clone();
            if let Some(turn) = best_light_diarization_turn(segment, turns) {
                let idx = light_speaker_index(&turn.speaker_key, &mut speaker_order);
                next.speaker_id = format!("speaker_{idx}");
                next.speaker_name = format!("Speaker {idx}");
            }
            next
        })
        .collect()
}

fn best_light_diarization_turn<'a>(
    segment: &MeetingTranscriptSegment,
    turns: &'a [LightDiarizationTurn],
) -> Option<&'a LightDiarizationTurn> {
    let segment_start = segment.start_ms;
    let segment_end = segment.end_ms.max(segment.start_ms + 1);
    let segment_duration = segment_end.saturating_sub(segment_start).max(1);
    let min_overlap = (segment_duration / 10).clamp(120, 900);
    turns
        .iter()
        .filter_map(|turn| {
            let overlap =
                interval_overlap_ms(segment_start, segment_end, turn.start_ms, turn.end_ms);
            (overlap >= min_overlap).then_some((turn, overlap))
        })
        .max_by(|(left_turn, left_overlap), (right_turn, right_overlap)| {
            left_overlap
                .cmp(right_overlap)
                .then_with(|| left_turn.confidence.total_cmp(&right_turn.confidence))
        })
        .map(|(turn, _)| turn)
}

fn interval_overlap_ms(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> u64 {
    left_end
        .min(right_end)
        .saturating_sub(left_start.max(right_start))
}

fn light_speaker_index(
    speaker_key: &str,
    speaker_order: &mut std::collections::HashMap<String, usize>,
) -> usize {
    if let Some(idx) = speaker_order.get(speaker_key) {
        return *idx;
    }
    let idx = speaker_order.len() + 1;
    speaker_order.insert(speaker_key.to_string(), idx);
    idx
}

fn write_light_final_diarization_outputs(
    final_paths: &FinalDiarizationPaths,
    mut transcript_artifact: MeetingTranscriptArtifact,
    segments: Vec<MeetingTranscriptSegment>,
) -> Result<(), String> {
    let transcript = format_meeting_timeline_transcript(&segments);
    transcript_artifact.status = "completed".to_string();
    transcript_artifact.diarization_json_path =
        Some(final_paths.diarization_json.to_string_lossy().to_string());
    transcript_artifact.final_diarization_json_path =
        Some(final_paths.diarization_json.to_string_lossy().to_string());
    transcript_artifact.final_transcript_json_path =
        Some(final_paths.transcript_json.to_string_lossy().to_string());
    transcript_artifact.transcript = transcript.clone();
    transcript_artifact.segments = segments.clone();
    transcript_artifact.generated_at_ms = now_ms();
    transcript_artifact.error = None;

    let transcript_json = serde_json::to_vec_pretty(&transcript_artifact)
        .map_err(|e| format!("failed to serialize light diarized transcript: {e}"))?;
    write_atomic(&final_paths.transcript_json, transcript_json).map_err(|e| {
        format!(
            "failed to write light diarized transcript {}: {e}",
            final_paths.transcript_json.display()
        )
    })?;
    write_atomic(
        &final_paths.transcript_json.with_extension("txt"),
        transcript.into_bytes(),
    )
    .map_err(|e| format!("failed to write light diarized transcript text: {e}"))?;

    let speakers = diarization_speakers_from_segments(&segments);
    let diarization_segments = segments
        .iter()
        .map(|segment| MeetingDiarizationSegment {
            speaker_id: segment.speaker_id.clone(),
            speaker_name: segment.speaker_name.clone(),
            source: segment.source.clone(),
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            confidence: if segment.speaker_id == "you" {
                0.95
            } else {
                0.82
            },
            method: LIGHT_DIARIZATION_PROVIDER.to_string(),
        })
        .collect();
    let artifact = MeetingDiarizationArtifact {
        schema_version: 1,
        status: "completed".to_string(),
        method: LIGHT_DIARIZATION_PROVIDER.to_string(),
        speakers,
        segments: diarization_segments,
        generated_at_ms: now_ms(),
        error: None,
    };
    let diarization_json = serde_json::to_vec_pretty(&artifact)
        .map_err(|e| format!("failed to serialize light diarization: {e}"))?;
    write_atomic(&final_paths.diarization_json, diarization_json).map_err(|e| {
        format!(
            "failed to write light diarization {}: {e}",
            final_paths.diarization_json.display()
        )
    })
}

fn write_final_diarization_failure(path: &Path, provider: &str, error: &str) {
    let artifact = MeetingDiarizationArtifact {
        schema_version: 1,
        status: "failed".to_string(),
        method: provider.to_string(),
        speakers: Vec::new(),
        segments: Vec::new(),
        generated_at_ms: now_ms(),
        error: Some(error.to_string()),
    };

    match serde_json::to_vec_pretty(&artifact) {
        Ok(json) => {
            if let Err(e) = write_atomic(path, json) {
                tracing::warn!(error = %e, path = %path.display(), "[meeting_engine] failed to write final diarization failure json");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize final diarization failure json");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_transcript_artifact(
    paths: &TranscriptPaths,
    summary: &MicCaptureSummary,
    status: &str,
    config: Option<&WhisperCppConfig>,
    language: &str,
    transcript: &str,
    latency_ms: Option<u64>,
    cleaned_transcript: Option<&str>,
    cleanup: MeetingCleanupSnapshot,
    segments: Vec<MeetingTranscriptSegment>,
    source_wavs: Vec<PathBuf>,
    error: Option<String>,
) {
    let fallback_transcript = filter_non_speech_transcript_lines(transcript);
    let segments = if segments.is_empty() && !fallback_transcript.trim().is_empty() {
        vec![MeetingTranscriptSegment {
            source: transcript_source(&summary.path).to_string(),
            speaker_id: transcript_source_id(&summary.path).to_string(),
            speaker_name: transcript_source_name(&summary.path).to_string(),
            start_ms: 0,
            end_ms: summary.duration_ms,
            text: fallback_transcript,
        }]
    } else {
        segments
    };
    let source_wavs = if source_wavs.is_empty() {
        vec![summary.path.to_string_lossy().to_string()]
    } else {
        source_wavs
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect()
    };
    let diarization_json_path = diarization_path_for_transcript(paths);
    let artifact = MeetingTranscriptArtifact {
        schema_version: 1,
        provider: "whisper.cpp".to_string(),
        status: status.to_string(),
        language: Some(language.to_string()),
        model: config.map(|config| config.model.to_string_lossy().to_string()),
        source_wav: summary.path.to_string_lossy().to_string(),
        source_wavs,
        diarization_json_path: diarization_json_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        final_diarization_json_path: None,
        final_transcript_json_path: None,
        transcript: transcript.to_string(),
        cleaned_transcript: cleaned_transcript.map(|text| text.to_string()),
        cleanup_status: cleanup.status,
        cleanup_provider: cleanup.provider,
        cleanup_model: cleanup.model,
        cleanup_latency_ms: cleanup.latency_ms,
        cleanup_error: cleanup.error,
        segments: segments.clone(),
        audio_duration_ms: summary.duration_ms,
        samples_written: summary.samples_written,
        latency_ms,
        generated_at_ms: now_ms(),
        error: error.clone(),
    };

    match serde_json::to_vec_pretty(&artifact) {
        Ok(json) => {
            if let Err(e) = write_atomic(&paths.json, json) {
                tracing::warn!(error = %e, path = %paths.json.display(), "[meeting_engine] failed to write transcript json");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize transcript json");
        }
    }

    if let Some(path) = diarization_json_path {
        write_diarization_artifact(&path, &segments, error);
    }
}

fn diarization_path_for_transcript(paths: &TranscriptPaths) -> Option<PathBuf> {
    let parent = paths.json.parent()?;
    let file_name = paths.json.file_name()?.to_str()?;
    let stem = file_name.strip_suffix(".transcript.json")?;
    Some(parent.join(format!("{stem}.diarization.json")))
}

fn final_diarization_paths_for_transcript(
    paths: &TranscriptPaths,
) -> Option<FinalDiarizationPaths> {
    let parent = paths.json.parent()?;
    let file_name = paths.json.file_name()?.to_str()?;
    let stem = file_name.strip_suffix(".transcript.json")?;
    Some(FinalDiarizationPaths {
        diarization_json: parent.join(format!("{stem}.diarization.final.json")),
        transcript_json: parent.join(format!("{stem}.transcript.final.json")),
    })
}

fn write_diarization_artifact(
    path: &Path,
    transcript_segments: &[MeetingTranscriptSegment],
    error: Option<String>,
) {
    let speakers = diarization_speakers_from_segments(transcript_segments);
    let segments = transcript_segments
        .iter()
        .map(|segment| MeetingDiarizationSegment {
            speaker_id: segment.speaker_id.clone(),
            speaker_name: segment.speaker_name.clone(),
            source: segment.source.clone(),
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            confidence: source_label_confidence(&segment.source),
            method: "source_track_v1".to_string(),
        })
        .collect();
    let artifact = MeetingDiarizationArtifact {
        schema_version: 1,
        status: if error.is_some() {
            "failed".to_string()
        } else {
            "completed".to_string()
        },
        method: "source_track_v1".to_string(),
        speakers,
        segments,
        generated_at_ms: now_ms(),
        error,
    };

    match serde_json::to_vec_pretty(&artifact) {
        Ok(json) => {
            if let Err(e) = write_atomic(path, json) {
                tracing::warn!(error = %e, path = %path.display(), "[meeting_engine] failed to write legacy source diarization json");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize legacy source diarization json");
        }
    }
}

fn diarization_speakers_from_segments(
    segments: &[MeetingTranscriptSegment],
) -> Vec<MeetingDiarizationSpeaker> {
    let mut speakers = Vec::new();
    for segment in segments {
        if speakers
            .iter()
            .any(|speaker: &MeetingDiarizationSpeaker| speaker.speaker_id == segment.speaker_id)
        {
            continue;
        }
        speakers.push(MeetingDiarizationSpeaker {
            speaker_id: segment.speaker_id.clone(),
            speaker_name: segment.speaker_name.clone(),
            source: segment.source.clone(),
            role: match segment.source.as_str() {
                "mic" => "local_user",
                "system" => "remote_speaker",
                _ => "unknown",
            }
            .to_string(),
        });
    }
    speakers
}

fn source_label_confidence(source: &str) -> f32 {
    match source {
        "mic" => 0.95,
        "system" => 0.7,
        _ => 0.5,
    }
}

fn transcript_source(path: &Path) -> &'static str {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "system.wav")
    {
        "system"
    } else {
        "mic"
    }
}

fn transcript_source_id(path: &Path) -> &'static str {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "system.wav")
    {
        "speaker_1"
    } else {
        "you"
    }
}

fn transcript_source_name(path: &Path) -> &'static str {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "system.wav")
    {
        "Speaker 1"
    } else {
        "You"
    }
}

fn select_meeting_ai_transcript(state: &MeetingEngineState) -> Result<MeetingAiTranscript, String> {
    let transcription = state
        .transcription
        .lock()
        .map_err(|_| "meeting engine lock poisoned".to_string())?;
    select_meeting_ai_transcript_from_snapshot(&transcription)
}

fn select_meeting_ai_transcript_from_snapshot(
    transcription: &TranscriptionSnapshot,
) -> Result<MeetingAiTranscript, String> {
    for (source, text) in [
        ("final", transcription.final_text.as_deref()),
        ("cleaned", transcription.cleaned_text.as_deref()),
        ("raw", transcription.text.as_deref()),
    ] {
        if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
            return Ok(MeetingAiTranscript {
                source: source.to_string(),
                text: text.to_string(),
            });
        }
    }
    Err("meeting transcript is not ready".to_string())
}

/// Pick the transcript to analyse. When a meeting id is given, prefer that
/// meeting's own cached final transcript on disk (so generation works after the
/// live session has ended/cleared); otherwise use the live engine state.
fn resolve_intelligence_transcript(
    state: &MeetingEngineState,
    meeting_id: Option<&str>,
) -> Result<MeetingAiTranscript, String> {
    if let Some(id) = meeting_id {
        if let Ok(Some(artifacts)) = load_cached_meeting_artifacts(state, Some(id)) {
            let text = artifacts.transcript.trim();
            if !text.is_empty() {
                return Ok(MeetingAiTranscript {
                    source: "cached-final".to_string(),
                    text: text.to_string(),
                });
            }
        }
    }
    select_meeting_ai_transcript(state)
}

/// Where the generated `meeting.ai.json` should be written. With a meeting id we
/// target that meeting's own folder so the detail view (which reads strictly by
/// id) finds it; otherwise fall back to the active session's dir.
fn intelligence_target_dir(
    state: &MeetingEngineState,
    meeting_id: Option<&str>,
) -> Option<PathBuf> {
    if let Some(id) = meeting_id {
        if let Ok(dir) = meeting_dir_for_id(id) {
            return Some(dir);
        }
    }
    meeting_intelligence_artifact_dirs(state, None)
        .into_iter()
        .next()
}

/// Blocking LLM work for meeting intelligence — takes owned inputs so it can run
/// on a blocking thread off the main/UI thread.
fn run_meeting_intelligence(
    selected: MeetingAiTranscript,
    target_dir: Option<PathBuf>,
) -> Result<MeetingIntelligenceResult, String> {
    let config = meeting_ai_config()?;
    let user_prompt = format!(
        "Transcript source: {}\n\nTranscript:\n<<<TRANSCRIPT\n{}\nTRANSCRIPT>>>",
        selected.source, selected.text
    );
    let completion = complete_meeting_llm(
        MEETING_INTELLIGENCE_SYSTEM_PROMPT,
        &user_prompt,
        config.clone(),
        meeting_ai_timeout(),
        meeting_ai_max_tokens(),
    )?;
    let completion = if meeting_ai_verification_enabled() {
        let draft_json = completion.content.clone();
        verify_meeting_intelligence(&selected.text, &draft_json, config, completion)?
    } else {
        completion
    };
    let (title, tags, summary, action_items, decisions) =
        parse_meeting_intelligence(&completion.content, Some(&selected.text))?;
    let result = MeetingIntelligenceResult {
        status: "completed".to_string(),
        provider: completion.provider,
        model: completion.model,
        latency_ms: completion.latency_ms,
        transcript_source: selected.source,
        title,
        tags,
        summary,
        action_items,
        decisions,
    };
    if let Some(dir) = target_dir {
        write_meeting_intelligence_cache(&dir, &result);
    }
    Ok(result)
}

fn load_cached_meeting_intelligence(
    state: &MeetingEngineState,
    meeting_id: Option<&str>,
) -> Result<Option<MeetingIntelligenceResult>, String> {
    for artifact_dir in meeting_intelligence_artifact_dirs(state, meeting_id) {
        // When a meeting id is given, only read that meeting's own folder so a
        // meeting never displays another meeting's cached summary/title/tags.
        if meeting_id.is_some_and(|id| !artifact_dir_matches_meeting_id(&artifact_dir, id)) {
            continue;
        }
        if let Some(result) = load_cached_meeting_intelligence_from_dir(&artifact_dir)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

fn load_cached_meeting_artifacts(
    state: &MeetingEngineState,
    meeting_id: Option<&str>,
) -> Result<Option<MeetingCachedArtifacts>, String> {
    for artifact_dir in meeting_intelligence_artifact_dirs(state, meeting_id) {
        if meeting_id.is_some_and(|id| !artifact_dir_matches_meeting_id(&artifact_dir, id)) {
            continue;
        }
        if let Some(artifacts) = load_cached_meeting_artifacts_from_dir(meeting_id, &artifact_dir)?
        {
            return Ok(Some(artifacts));
        }
    }
    Ok(None)
}

fn artifact_dir_matches_meeting_id(artifact_dir: &Path, meeting_id: &str) -> bool {
    artifact_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == meeting_id)
}

fn load_cached_meeting_artifacts_from_dir(
    meeting_id: Option<&str>,
    artifact_dir: &Path,
) -> Result<Option<MeetingCachedArtifacts>, String> {
    if !artifact_dir.is_dir() {
        return Ok(None);
    }

    let transcript_candidates = [
        artifact_dir.join("meeting.transcript.final.json"),
        artifact_dir.join("meeting.transcript.json"),
        artifact_dir.join("meeting.merged.transcript.json"),
        artifact_dir.join("mic.transcript.json"),
        artifact_dir.join("system.transcript.json"),
    ];
    let transcript_text_candidates = [
        artifact_dir.join("meeting.transcript.final.txt"),
        artifact_dir.join("meeting.transcript.txt"),
        artifact_dir.join("meeting.merged.transcript.txt"),
        artifact_dir.join("mic.transcript.txt"),
        artifact_dir.join("system.transcript.txt"),
    ];

    let mut transcript_path = None;
    let mut transcript_source = "none".to_string();
    let mut transcript = String::new();
    let mut segments = Vec::new();
    let mut audio_duration_ms = None;
    let mut source_audio_paths = Vec::new();

    for path in transcript_candidates {
        if !path.is_file() {
            continue;
        }
        let parsed = load_cached_transcript_artifact(&path)?;
        transcript_path = Some(path.to_string_lossy().to_string());
        transcript_source = cached_transcript_source_label(&path).to_string();
        transcript = parsed.transcript;
        segments = parsed.segments;
        audio_duration_ms = parsed.audio_duration_ms;
        source_audio_paths = parsed.source_audio_paths;
        break;
    }

    if transcript.trim().is_empty() {
        for path in transcript_text_candidates {
            if !path.is_file() {
                continue;
            }
            let raw = fs::read_to_string(&path).map_err(|e| {
                format!(
                    "failed to read cached meeting transcript {}: {e}",
                    path.display()
                )
            })?;
            let raw = raw.trim().to_string();
            if raw.is_empty() {
                continue;
            }
            transcript_path = Some(path.to_string_lossy().to_string());
            transcript_source = cached_transcript_source_label(&path).to_string();
            transcript = raw;
            segments = parse_timeline_transcript_text(&transcript);
            break;
        }
    }

    let audio_path = choose_cached_meeting_audio_path(artifact_dir, &source_audio_paths);
    if audio_duration_ms.is_none() {
        audio_duration_ms = audio_path.as_deref().and_then(wav_duration_ms);
    }

    if transcript.trim().is_empty() && audio_path.is_none() {
        return Ok(None);
    }

    Ok(Some(MeetingCachedArtifacts {
        meeting_id: meeting_id.map(ToString::to_string),
        artifact_dir: artifact_dir.to_string_lossy().to_string(),
        audio_path: audio_path.map(|path| path.to_string_lossy().to_string()),
        audio_duration_ms,
        transcript_path,
        transcript_source,
        transcript,
        segments,
    }))
}

struct CachedTranscriptArtifact {
    transcript: String,
    segments: Vec<MeetingCachedTranscriptSegment>,
    audio_duration_ms: Option<u64>,
    source_audio_paths: Vec<PathBuf>,
}

fn load_cached_transcript_artifact(path: &Path) -> Result<CachedTranscriptArtifact, String> {
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "failed to read cached meeting transcript {}: {e}",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "failed to parse cached meeting transcript {}: {e}",
            path.display()
        )
    })?;

    let transcript = value
        .get("transcript")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("text").and_then(|value| value.as_str()))
        .or_else(|| {
            value
                .get("cleaned_transcript")
                .and_then(|value| value.as_str())
        })
        .unwrap_or("")
        .trim()
        .to_string();

    let mut segments = value
        .get("segments")
        .and_then(|value| value.as_array())
        .map(|segments| {
            segments
                .iter()
                .filter_map(parse_cached_transcript_segment)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    segments.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| left.end_ms.cmp(&right.end_ms))
            .then_with(|| left.speaker_id.cmp(&right.speaker_id))
    });

    let mut source_audio_paths = Vec::new();
    if let Some(path) = value.get("source_wav").and_then(|value| value.as_str()) {
        source_audio_paths.push(PathBuf::from(path));
    }
    if let Some(paths) = value.get("source_wavs").and_then(|value| value.as_array()) {
        source_audio_paths.extend(
            paths
                .iter()
                .filter_map(|value| value.as_str())
                .map(PathBuf::from),
        );
    }

    Ok(CachedTranscriptArtifact {
        transcript: if transcript.is_empty() && !segments.is_empty() {
            format_cached_segments_as_transcript(&segments)
        } else {
            transcript
        },
        segments,
        audio_duration_ms: value
            .get("audio_duration_ms")
            .and_then(|value| value.as_u64())
            .or_else(|| value.get("duration_ms").and_then(|value| value.as_u64())),
        source_audio_paths,
    })
}

fn parse_cached_transcript_segment(
    value: &serde_json::Value,
) -> Option<MeetingCachedTranscriptSegment> {
    let text = value.get("text")?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let start_ms = cached_segment_timestamp_ms(
        value,
        &[
            "display_start_ms",
            "speech_start_ms",
            "transcript_start_ms",
            "start_ms",
        ],
    )
    .unwrap_or(0);
    let end_ms = cached_segment_timestamp_ms(
        value,
        &[
            "display_end_ms",
            "speech_end_ms",
            "transcript_end_ms",
            "end_ms",
        ],
    )
    .unwrap_or(start_ms);
    let source = value
        .get("source")
        .and_then(|value| value.as_str())
        .unwrap_or("meeting")
        .to_string();
    let speaker_id = value
        .get("speaker_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| if source == "mic" { "you" } else { "speaker_1" })
        .to_string();
    let speaker_name = value
        .get("speaker_name")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if speaker_id == "you" {
                "You"
            } else {
                "Speaker 1"
            }
        })
        .to_string();

    Some(MeetingCachedTranscriptSegment {
        source,
        speaker_id,
        speaker_name,
        start_ms,
        end_ms: end_ms.max(start_ms),
        text: text.to_string(),
    })
}

fn cached_segment_timestamp_ms(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|value| value.as_u64()))
}

fn parse_timeline_transcript_text(transcript: &str) -> Vec<MeetingCachedTranscriptSegment> {
    transcript
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let captures = line.strip_prefix('[')?.split_once(']')?;
            let (header, text) = captures;
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            let (time, speaker) = header.split_once(' ')?;
            let start_ms = parse_cached_timestamp_to_ms(time)?;
            let speaker_name = speaker.trim().to_string();
            Some(MeetingCachedTranscriptSegment {
                source: "meeting".to_string(),
                speaker_id: speaker_name
                    .to_lowercase()
                    .replace(|ch: char| !ch.is_ascii_alphanumeric(), "_"),
                speaker_name,
                start_ms,
                end_ms: start_ms,
                text: text.to_string(),
            })
        })
        .collect()
}

fn parse_cached_timestamp_to_ms(timestamp: &str) -> Option<u64> {
    let parts = timestamp.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [minutes, seconds] => {
            Some((minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?) * 1000)
        }
        [hours, minutes, seconds] => Some(
            (hours.parse::<u64>().ok()? * 3600
                + minutes.parse::<u64>().ok()? * 60
                + seconds.parse::<u64>().ok()?)
                * 1000,
        ),
        _ => None,
    }
}

fn format_cached_segments_as_transcript(segments: &[MeetingCachedTranscriptSegment]) -> String {
    segments
        .iter()
        .map(|segment| {
            format!(
                "[{} {}] {}",
                format_timestamp_ms(segment.start_ms),
                segment.speaker_name,
                segment.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cached_transcript_source_label(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name.contains(".final.") {
        "final"
    } else if name.contains(".merged.") {
        "merged"
    } else if name.starts_with("mic.") || name.starts_with("system.") {
        "track"
    } else {
        "raw"
    }
}

fn choose_cached_meeting_audio_path(
    artifact_dir: &Path,
    source_audio_paths: &[PathBuf],
) -> Option<PathBuf> {
    [
        artifact_dir.join("meeting.merged.wav"),
        artifact_dir.join("meeting.wav"),
        artifact_dir.join("audio.wav"),
        artifact_dir.join("mic.wav"),
        artifact_dir.join("system.wav"),
    ]
    .into_iter()
    .chain(source_audio_paths.iter().cloned())
    .find(|path| path.is_file())
}

fn wav_duration_ms(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut riff = [0_u8; 12];
    file.read_exact(&mut riff).ok()?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return None;
    }

    let mut channels = None;
    let mut sample_rate = None;
    let mut bits_per_sample = None;
    let mut data_bytes = None;

    loop {
        let mut header = [0_u8; 8];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let chunk_id = &header[0..4];
        let chunk_size = u32::from_le_bytes(header[4..8].try_into().ok()?) as u64;
        let padded_size = chunk_size + (chunk_size % 2);
        if chunk_id == b"fmt " {
            let mut fmt = vec![0_u8; chunk_size as usize];
            file.read_exact(&mut fmt).ok()?;
            if fmt.len() >= 16 {
                channels = Some(u16::from_le_bytes([fmt[2], fmt[3]]) as u64);
                sample_rate = Some(u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]) as u64);
                bits_per_sample = Some(u16::from_le_bytes([fmt[14], fmt[15]]) as u64);
            }
            if padded_size > chunk_size {
                file.seek(SeekFrom::Current((padded_size - chunk_size) as i64))
                    .ok()?;
            }
        } else if chunk_id == b"data" {
            data_bytes = Some(chunk_size);
            break;
        } else {
            file.seek(SeekFrom::Current(padded_size as i64)).ok()?;
        }
    }

    let bytes_per_second = sample_rate? * channels? * (bits_per_sample? / 8).max(1);
    if bytes_per_second == 0 {
        return None;
    }
    Some(data_bytes? * 1000 / bytes_per_second)
}

fn meeting_intelligence_artifact_dirs(
    state: &MeetingEngineState,
    meeting_id: Option<&str>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(meeting_id) = meeting_id
        .map(str::trim)
        .filter(|meeting_id| !meeting_id.is_empty())
    {
        dirs.push(
            said_core::paths::data_dir()
                .join("meetings")
                .join(meeting_id),
        );
    }

    if let Some(session) = state.session.lock_recover().clone() {
        dirs.push(session.artifact_dir);
    }

    let transcription = state.transcription.lock_recover().clone();
    for path in [
        transcription.final_diarization.transcript_json_path,
        transcription.final_diarization.diarization_json_path,
        transcription.json_path,
        transcription.text_path,
    ]
    .into_iter()
    .flatten()
    {
        if let Some(parent) = path.parent() {
            dirs.push(parent.to_path_buf());
        }
    }

    dirs.extend(recent_meeting_artifact_dirs(50));

    dedupe_paths(dirs)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if deduped.iter().any(|existing: &PathBuf| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    deduped
}

fn recent_meeting_artifact_dirs(limit: usize) -> Vec<PathBuf> {
    let root = said_core::paths::data_dir().join("meetings");
    let Some(entries) = fs::read_dir(root).ok() else {
        return Vec::new();
    };
    let mut dirs = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            Some((modified, path))
        })
        .collect::<Vec<_>>();
    dirs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    dirs.into_iter().take(limit).map(|(_, path)| path).collect()
}

fn load_cached_meeting_intelligence_from_dir(
    artifact_dir: &Path,
) -> Result<Option<MeetingIntelligenceResult>, String> {
    let candidates = [
        artifact_dir.join("meeting.ai.json"),
        artifact_dir.join("meeting.mom.json"),
        artifact_dir
            .join("meeting-ai-manual")
            .join("latest.meeting-ai.json"),
        artifact_dir.join("meeting-ai-manual").join("summary.json"),
    ];

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        if let Some(result) = load_cached_meeting_intelligence_file(&path)? {
            return Ok(Some(result));
        }
    }

    Ok(None)
}

fn load_cached_meeting_intelligence_file(
    path: &Path,
) -> Result<Option<MeetingIntelligenceResult>, String> {
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "failed to read cached meeting intelligence {}: {e}",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "failed to parse cached meeting intelligence {}: {e}",
            path.display()
        )
    })?;

    parse_cached_meeting_intelligence_value(&value).map_err(|e| {
        format!(
            "failed to load cached meeting intelligence {}: {e}",
            path.display()
        )
    })
}

fn parse_cached_meeting_intelligence_value(
    value: &serde_json::Value,
) -> Result<Option<MeetingIntelligenceResult>, String> {
    let record = match value {
        serde_json::Value::Array(records) => records
            .iter()
            .rev()
            .find(|record| record.is_object())
            .ok_or_else(|| "cache array did not contain an object".to_string())?,
        serde_json::Value::Object(_) => value,
        _ => return Ok(None),
    };

    if record.get("filtered_mom").is_some() {
        return parse_bench_meeting_intelligence_record(record).map(Some);
    }

    if record.get("summary").is_some()
        && record.get("action_items").is_some()
        && record.get("decisions").is_some()
    {
        let result: MeetingIntelligenceResult = serde_json::from_value(record.clone())
            .map_err(|e| format!("cached native meeting intelligence is invalid: {e}"))?;
        return Ok(Some(result));
    }

    Ok(None)
}

fn parse_bench_meeting_intelligence_record(
    record: &serde_json::Value,
) -> Result<MeetingIntelligenceResult, String> {
    let mom = record
        .get("filtered_mom")
        .ok_or_else(|| "missing filtered_mom".to_string())?;
    let mom_json =
        serde_json::to_string(mom).map_err(|e| format!("failed to serialize filtered_mom: {e}"))?;
    let (title, tags, summary, action_items, decisions) =
        parse_meeting_intelligence(&mom_json, None)?;
    let provider = record
        .get("provider")
        .and_then(|value| value.as_str())
        .unwrap_or("cached")
        .to_string();
    let model = record
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("meeting-ai")
        .to_string();
    let latency_ms = record
        .get("draft_latency_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or_default()
        .saturating_add(
            record
                .get("verify_latency_ms")
                .and_then(|value| value.as_u64())
                .unwrap_or_default(),
        );

    Ok(MeetingIntelligenceResult {
        status: "completed".to_string(),
        provider,
        model,
        latency_ms,
        transcript_source: "cached-final".to_string(),
        title,
        tags,
        summary,
        action_items,
        decisions,
    })
}

fn write_meeting_intelligence_cache(artifact_dir: &Path, result: &MeetingIntelligenceResult) {
    if !artifact_dir.is_dir() {
        return;
    }
    let path = artifact_dir.join("meeting.ai.json");
    match serde_json::to_vec_pretty(result) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(&path, bytes) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "[meeting_engine] failed to write meeting intelligence cache"
                );
            } else {
                // Checkpoint: the meeting is fully processed end to end.
                write_meeting_state(artifact_dir, MEETING_PHASE_SUMMARIZED, None);
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "[meeting_engine] failed to serialize meeting intelligence cache"
            );
        }
    }
}

fn verify_meeting_intelligence(
    transcript: &str,
    draft_json: &str,
    config: MeetingCleanupConfig,
    draft_completion: MeetingLlmCompletion,
) -> Result<MeetingLlmCompletion, String> {
    let user_prompt = format!(
        "Transcript:\n<<<TRANSCRIPT\n{}\nTRANSCRIPT>>>\n\nDraft JSON:\n<<<JSON\n{}\nJSON>>>",
        transcript, draft_json
    );
    let verified = complete_meeting_llm(
        MEETING_INTELLIGENCE_VERIFIER_SYSTEM_PROMPT,
        &user_prompt,
        config,
        meeting_ai_timeout(),
        meeting_ai_max_tokens(),
    )?;

    Ok(MeetingLlmCompletion {
        content: verified.content,
        provider: verified.provider,
        model: verified.model,
        latency_ms: draft_completion
            .latency_ms
            .saturating_add(verified.latency_ms),
    })
}

/// Pick the transcript a chat question should run against. Uses the caller's
/// override when present, otherwise reads the engine's selected transcript.
/// Kept separate from [`answer_meeting_question`] so the state read happens on
/// the command task while the blocking LLM call runs on a blocking thread.
fn resolve_meeting_chat_transcript(
    state: &MeetingEngineState,
    transcript_override: Option<&str>,
) -> Result<MeetingAiTranscript, String> {
    transcript_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|text| MeetingAiTranscript {
            source: "live".to_string(),
            text: text.to_string(),
        })
        .map(Ok)
        .unwrap_or_else(|| select_meeting_ai_transcript(state))
}

/// Char budget for the transcript portion of a chat prompt. Transcripts under
/// this pass through whole (long-context path); larger ones are assembled with
/// retrieval-lite so a multi-hour meeting never overflows the model context.
/// ~48K chars ≈ 12K tokens, comfortably inside the cleanup/AI model windows.
const MEETING_CHAT_TRANSCRIPT_CHAR_BUDGET: usize = 48_000;

/// Lowercase alphanumeric tokens (length > 2) for BM25-lite scoring.
fn chat_tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(|t| t.to_string())
        .collect()
}

/// Assemble the transcript context for a chat question within `budget` chars.
///
/// - Small transcript → returned whole (long-context path).
/// - Large transcript → retrieval-lite: split into per-line segments, score each
///   against the question with BM25-lite (TF + IDF), then include a recency tail
///   (most recent segments) plus the highest-scoring relevant segments, emitted
///   in chronological order with `[…]` gap markers. Zero external deps; never
///   exceeds the budget. Existing `[mm:ss Speaker]` line prefixes are preserved
///   so the model can still cite timestamps.
fn assemble_chat_transcript_context(transcript: &str, question: &str, budget: usize) -> String {
    let transcript = transcript.trim();
    if transcript.len() <= budget {
        return transcript.to_string();
    }
    let segments: Vec<&str> = transcript
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    // Degenerate (one giant blob / no line structure): keep the recent tail.
    if segments.len() <= 2 {
        let start = transcript.len().saturating_sub(budget);
        return format!("[…]\n{}", &transcript[start..]);
    }

    let seg_tokens: Vec<Vec<String>> = segments.iter().map(|s| chat_tokenize(s)).collect();
    let mut doc_freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for tokens in &seg_tokens {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for token in tokens {
            if seen.insert(token.as_str()) {
                *doc_freq.entry(token.as_str()).or_insert(0) += 1;
            }
        }
    }
    let n = segments.len() as f64;
    let q_terms = chat_tokenize(question);

    let mut included: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut used = 0usize;

    // 1) Recency tail — always keep the most recent ~20% of the budget so the
    //    model knows what was *just* said (the most common live-chat intent).
    let recency_budget = budget / 5;
    for i in (0..segments.len()).rev() {
        let cost = segments[i].len() + 1;
        if used + cost > recency_budget {
            break;
        }
        included.insert(i);
        used += cost;
    }

    // 2) Relevance — fill the rest with the highest-scoring segments.
    let mut scored: Vec<(usize, f64)> = seg_tokens
        .iter()
        .enumerate()
        .map(|(i, tokens)| {
            let mut score = 0.0;
            for qt in &q_terms {
                let tf = tokens.iter().filter(|t| t.as_str() == qt).count() as f64;
                if tf > 0.0 {
                    let df = *doc_freq.get(qt.as_str()).unwrap_or(&1) as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    score += idf * (tf / (tf + 1.0));
                }
            }
            (i, score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, score) in scored {
        if score <= 0.0 || used >= budget {
            break;
        }
        if included.contains(&i) {
            continue;
        }
        let cost = segments[i].len() + 1;
        if used + cost > budget {
            continue;
        }
        included.insert(i);
        used += cost;
    }

    // Emit chronologically with gap markers so the model knows context is sparse.
    let mut out = String::new();
    let mut prev: Option<usize> = None;
    for &i in &included {
        if prev.map(|p| i > p + 1).unwrap_or(i > 0) {
            out.push_str("[…]\n");
        }
        out.push_str(segments[i]);
        out.push('\n');
        prev = Some(i);
    }
    out
}

fn answer_meeting_question(
    selected: MeetingAiTranscript,
    question: &str,
    summary: Option<&str>,
    notes: Option<&str>,
    on_delta: impl FnMut(&str),
) -> Result<MeetingChatResult, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("question is empty".to_string());
    }

    let config = meeting_ai_config()?;
    tracing::info!(
        provider = %config.provider,
        model = %config.model,
        transcript_source = %selected.source,
        transcript_chars = selected.text.len(),
        "[meeting_engine] meeting chat request started"
    );
    let summary = summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No generated summary is available yet.");
    // The user's own notes are high-signal context — fold them in when present.
    let notes_section = notes
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|notes| {
            format!("\n\nUser's personal notes (treat as high-priority context):\n<<<NOTES\n{notes}\nNOTES>>>")
        })
        .unwrap_or_default();
    // Budget-aware assembly: whole transcript when it fits, else retrieval-lite
    // (summary + most-relevant segments + recency tail) so long meetings never
    // overflow the model context.
    let budget = env_u64(
        "AIRNOTE_MEETING_CHAT_TRANSCRIPT_CHAR_BUDGET",
        MEETING_CHAT_TRANSCRIPT_CHAR_BUDGET as u64,
    ) as usize;
    let transcript_context = assemble_chat_transcript_context(&selected.text, question, budget);
    let excerpted = transcript_context.len() < selected.text.trim().len();
    let transcript_note = if excerpted {
        "\n(The transcript below is EXCERPTED to the most relevant moments for this question; `[…]` marks omitted spans. Rely on the summary for overall context and say so if the excerpt is insufficient.)"
    } else {
        ""
    };
    let user_prompt = format!(
        "Transcript source: {}\n\nMeeting intelligence:\n<<<SUMMARY\n{}\nSUMMARY>>>{}\n\nTranscript:{}\n<<<TRANSCRIPT\n{}\nTRANSCRIPT>>>\n\nQuestion:\n{}",
        selected.source, summary, notes_section, transcript_note, transcript_context, question
    );
    let completion = complete_meeting_llm_streaming(
        MEETING_CHAT_SYSTEM_PROMPT,
        &user_prompt,
        config,
        meeting_chat_timeout(),
        meeting_ai_max_tokens(),
        on_delta,
    )
    .inspect_err(|e| {
        tracing::warn!(error = %e, "[meeting_engine] meeting chat request failed");
    })?;
    let answer = strip_llm_code_fences(&completion.content);
    if answer.trim().is_empty() {
        tracing::warn!("[meeting_engine] meeting chat returned an empty answer");
        return Err("meeting chat returned an empty answer".to_string());
    }
    tracing::info!(
        latency_ms = completion.latency_ms,
        answer_chars = answer.len(),
        "[meeting_engine] meeting chat request completed"
    );

    Ok(MeetingChatResult {
        status: "completed".to_string(),
        provider: completion.provider,
        model: completion.model,
        latency_ms: completion.latency_ms,
        transcript_source: selected.source,
        answer,
    })
}

// ============================ Cross-meeting digest ============================
//
// A "digest" synthesizes ONE combined report across many meetings (a multi-select
// or a date range). It runs entirely on the desktop: per-meeting summaries and
// transcripts live as local files, so we read them here and reuse the same LLM
// helpers as single-meeting intelligence/chat. Selection (ids/titles/dates) comes
// from the caller (the cloud meeting list).

/// Synthesis input budget (chars). Per-meeting summaries are packed up to this;
/// larger selections fall back to map-reduce (summarize batches, then merge).
const DEFAULT_MEETING_DIGEST_INPUT_CHAR_BUDGET: usize = 120_000;
/// Per-meeting summary cap (chars) inside the synthesis input so one giant MoM
/// can't crowd out the others.
const DEFAULT_DIGEST_MEETING_SUMMARY_CAP: usize = 6_000;
/// Char budget for the pooled transcript excerpts in a digest chat turn.
const DEFAULT_MEETING_DIGEST_CHAT_CHAR_BUDGET: usize = 60_000;

const MEETING_DIGEST_SYSTEM_PROMPT: &str = r####"You are AirNote's cross-meeting digest engine.

You are given material from MULTIPLE meetings. Each block starts with "### <title> (<date>)" and contains that meeting's Summary, Decisions, and Action items. Synthesize ONE combined digest across all of them.

Use only the supplied material. Do not invent meetings, people, decisions, dates, or action items. If the material is itself a set of partial digests, merge them faithfully.

Produce:
- "title": a concise, specific heading (3-8 words) naming the overall theme of the period (e.g. "Sentinel Rollout & Pricing Week"). No dates, no quotes, no trailing period. Never generic like "Meeting notes".
- "executive_summary": clean Markdown plain text (no HTML). A tight, client-ready overview of the whole period: what was worked on across the meetings, how topics progressed, what was decided, and what is still open. Use short paragraphs and "- " bullets. Connect related points ACROSS meetings when the material supports it. Do not merely concatenate per-meeting summaries.
- "themes": recurring topics that span the meetings. Each: { "title", "detail" (1-3 sentences), "meetings": [titles that touched this theme] }. Cluster related discussion; omit one-off trivia. Roughly 3-7 themes for a rich set, fewer for a small one.
- "decisions": de-duplicated decisions across all meetings. Each: { "text", "meeting" (source meeting title), "date" }. Only explicit agreements or final choices. Merge duplicates that recur across meetings (keep the clearest wording, cite the earliest meeting).
- "action_items": de-duplicated action items across all meetings. Each: { "title", "owner" (person if explicitly named, else null), "meeting", "date" }. Only firm commitments. Merge duplicates.
- "trends": short bullets describing how things changed across the meetings (e.g. "Pricing discussed in 3 meetings, narrowed to per-seat"). Only when the material shows progression. May be empty.
- "open_items": unresolved questions or things still pending across the period. May be empty.

Return only valid JSON with this exact shape:
{
  "title": "concise specific period title, 3-8 words",
  "executive_summary": "Markdown-compatible overview with short paragraphs and bullets",
  "themes": [ { "title": "Theme", "detail": "1-3 sentences", "meetings": ["Meeting title"] } ],
  "decisions": [ { "text": "explicit decision", "meeting": "source meeting title", "date": "date if known else empty" } ],
  "action_items": [ { "title": "firm action", "owner": "person if explicit else null", "meeting": "source meeting title", "date": "date if known else empty" } ],
  "trends": ["short trend bullet"],
  "open_items": ["unresolved item"]
}"####;

const MEETING_DIGEST_VERIFIER_SYSTEM_PROMPT: &str = r####"You are AirNote's strict cross-meeting digest verifier.

Use only the supplied source material and draft JSON. Return only valid JSON with the same shape as the draft.

Rules:
- Keep the "title" concise (3-8 words) and specific; replace it only if generic, inaccurate, or unsupported.
- Keep the executive_summary detailed and Markdown-formatted; remove any claim, decision, risk, or expectation not supported by the source material. Do not collapse it into one line.
- Keep a decision only if the source shows explicit agreement or a final choice; merge duplicates; preserve its source-meeting attribution.
- Keep an action item only if the source shows a firm commitment; set "owner" to null unless a person is explicitly named; merge duplicates.
- Drop themes, trends, and open_items not grounded in the source material.
- If uncertain about an item, remove it."####;

const MEETING_DIGEST_CHAT_SYSTEM_PROMPT: &str = r####"You are AirNote's cross-meeting Q&A engine.

Answer the user's question using ONLY the supplied cross-meeting digest summary and the per-meeting transcript excerpts. Each excerpt is grouped under "## <meeting title · date>"; lines may carry [mm:ss] timestamps and "[…]" marks omitted spans.
- Draw on multiple meetings when relevant, and compare or contrast them when the question asks.
- Always attribute facts to their meeting (cite the meeting title/date, and the timestamp when useful).
- If the answer is not present in the provided material, say so plainly. Do not infer owners, decisions, dates, or commitments beyond the material.
- Be concise and well-structured."####;

/// One meeting in a digest request, supplied by the caller (from the cloud list).
#[derive(Clone, Debug, Deserialize)]
pub struct DigestMeetingRef {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    date: String,
}

/// Per-meeting material gathered locally before synthesis.
#[derive(Clone, Debug)]
struct DigestCard {
    id: String,
    title: String,
    date: String,
    summary: String,
    decisions: Vec<String>,
    actions: Vec<(String, Option<String>)>,
}

// LLM synthesis output (deserialized from the model's JSON).
#[derive(Debug, Deserialize)]
struct DigestPayload {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    executive_summary: Option<String>,
    #[serde(default)]
    themes: Vec<DigestThemePayload>,
    #[serde(default)]
    decisions: Vec<DigestDecisionPayload>,
    #[serde(default)]
    action_items: Vec<DigestActionPayload>,
    #[serde(default)]
    trends: Vec<String>,
    #[serde(default)]
    open_items: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DigestThemePayload {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    meetings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DigestDecisionPayload {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    meeting: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DigestActionPayload {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    meeting: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

// Validated, serialized digest returned to the frontend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestTheme {
    title: String,
    detail: String,
    meetings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestDecisionItem {
    text: String,
    meeting: String,
    date: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestActionItem {
    title: String,
    owner: Option<String>,
    meeting: String,
    date: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestPerMeeting {
    id: String,
    title: String,
    date: String,
    recap: String,
    has_intelligence: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestSkipped {
    id: String,
    title: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DigestResult {
    /// Stable key for the selection (frontend cache + chat reset key).
    id: String,
    title: String,
    date_range: String,
    meeting_count: usize,
    included_meeting_ids: Vec<String>,
    skipped: Vec<DigestSkipped>,
    executive_summary: String,
    themes: Vec<DigestTheme>,
    decisions: Vec<DigestDecisionItem>,
    action_items: Vec<DigestActionItem>,
    trends: Vec<String>,
    open_items: Vec<String>,
    per_meeting: Vec<DigestPerMeeting>,
    /// Pre-rendered Markdown for Lark export / copy.
    markdown: String,
    provider: String,
    model: String,
    latency_ms: u64,
    /// When this digest was generated (ms since epoch); 0 for legacy snapshots.
    #[serde(default)]
    created_at: u64,
}

/// Truncate to `max_chars` on a char boundary, appending " …" when cut.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str(" …");
    out
}

/// A one-line recap of a meeting summary: flatten whitespace, then truncate.
fn digest_meeting_recap(summary: &str, max_chars: usize) -> String {
    let flat = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flat, max_chars)
}

/// "title · date" label used in prompts and chat grouping.
fn digest_meeting_label(r: &DigestMeetingRef) -> String {
    let title = if r.title.trim().is_empty() {
        "Untitled meeting"
    } else {
        r.title.trim()
    };
    if r.date.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title} · {}", r.date.trim())
    }
}

/// Date range string from the (chronologically ordered) refs.
fn digest_date_range(refs: &[DigestMeetingRef]) -> String {
    let dates: Vec<&str> = refs
        .iter()
        .map(|r| r.date.trim())
        .filter(|d| !d.is_empty())
        .collect();
    match (dates.first(), dates.last()) {
        (Some(f), Some(l)) if f == l => (*f).to_string(),
        (Some(f), Some(l)) => format!("{f} – {l}"),
        _ => String::new(),
    }
}

/// Order-independent cache key for a selection + missing-data strategy.
fn digest_cache_key(ids: &[String], missing: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for id in sorted {
        id.hash(&mut hasher);
    }
    missing.to_ascii_lowercase().hash(&mut hasher);
    format!("digest-{:016x}", hasher.finish())
}

/// Greedily pack blocks (by char length) into batches under `budget`.
fn pack_into_batches(block_lens: &[usize], budget: usize) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_len = 0usize;
    for (i, &len) in block_lens.iter().enumerate() {
        if !cur.is_empty() && cur_len + len > budget {
            batches.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        cur.push(i);
        cur_len += len;
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

/// Render one meeting as a synthesis-input block (summary capped).
fn build_meeting_block(card: &DigestCard, summary_cap: usize) -> String {
    let mut s = String::new();
    if card.date.trim().is_empty() {
        s.push_str(&format!("### {}\n", card.title));
    } else {
        s.push_str(&format!("### {} ({})\n", card.title, card.date));
    }
    s.push_str("Summary:\n");
    s.push_str(&truncate_chars(&card.summary, summary_cap));
    s.push('\n');
    if !card.decisions.is_empty() {
        s.push_str("Decisions:\n");
        for d in &card.decisions {
            let d = d.trim();
            if !d.is_empty() {
                s.push_str(&format!("- {d}\n"));
            }
        }
    }
    if !card.actions.is_empty() {
        s.push_str("Action items:\n");
        for (title, owner) in &card.actions {
            let title = title.trim();
            if title.is_empty() {
                continue;
            }
            match owner.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
                Some(o) => s.push_str(&format!("- {o} — {title}\n")),
                None => s.push_str(&format!("- {title}\n")),
            }
        }
    }
    s
}

/// Compact text rendering of a batch result, used as input to the merge pass.
fn render_partial_digest(p: &DigestPayload) -> String {
    let mut s = String::new();
    let title = p
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or("Partial digest");
    s.push_str(&format!("### Partial digest: {title}\n"));
    if let Some(exec) = p.executive_summary.as_deref().map(str::trim) {
        if !exec.is_empty() {
            s.push_str("Summary:\n");
            s.push_str(exec);
            s.push('\n');
        }
    }
    if !p.decisions.is_empty() {
        s.push_str("Decisions:\n");
        for d in &p.decisions {
            if let Some(t) = d.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                s.push_str(&format!("- {t}\n"));
            }
        }
    }
    if !p.action_items.is_empty() {
        s.push_str("Action items:\n");
        for a in &p.action_items {
            if let Some(t) = a.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                match a.owner.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
                    Some(o) => s.push_str(&format!("- {o} — {t}\n")),
                    None => s.push_str(&format!("- {t}\n")),
                }
            }
        }
    }
    s
}

/// Render the final digest as Markdown (consumed by Lark export + copy + the
/// in-app report). Avoids `_italics_` since the Lark converter only honors
/// `**bold**`, headings, and bullets.
fn render_digest_markdown(r: &DigestResult) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", r.title));
    let mut meta = String::new();
    if !r.date_range.is_empty() {
        meta.push_str(&r.date_range);
        meta.push_str(" · ");
    }
    meta.push_str(&format!(
        "{} meeting{}",
        r.meeting_count,
        if r.meeting_count == 1 { "" } else { "s" }
    ));
    s.push_str(&format!("{meta}\n\n"));
    if !r.skipped.is_empty() {
        s.push_str(&format!(
            "{} meeting{} skipped (not analyzed).\n\n",
            r.skipped.len(),
            if r.skipped.len() == 1 { "" } else { "s" }
        ));
    }
    if !r.executive_summary.is_empty() {
        s.push_str("## Executive Summary\n\n");
        s.push_str(r.executive_summary.trim());
        s.push_str("\n\n");
    }
    if !r.themes.is_empty() {
        s.push_str("## Key Themes\n\n");
        for t in &r.themes {
            s.push_str(&format!("### {}\n\n", t.title));
            if !t.detail.is_empty() {
                s.push_str(t.detail.trim());
                s.push_str("\n\n");
            }
            if !t.meetings.is_empty() {
                s.push_str(&format!("Meetings: {}\n\n", t.meetings.join(", ")));
            }
        }
    }
    let source_suffix = |meeting: &str, date: &str| -> String {
        let mut src = meeting.trim().to_string();
        if !date.trim().is_empty() {
            if src.is_empty() {
                src = date.trim().to_string();
            } else {
                src.push_str(&format!(", {}", date.trim()));
            }
        }
        src
    };
    if !r.decisions.is_empty() {
        s.push_str("## Decisions\n\n");
        for d in &r.decisions {
            let src = source_suffix(&d.meeting, &d.date);
            if src.is_empty() {
                s.push_str(&format!("- {}\n", d.text));
            } else {
                s.push_str(&format!("- {} — {}\n", d.text, src));
            }
        }
        s.push('\n');
    }
    if !r.action_items.is_empty() {
        s.push_str("## Action Items\n\n");
        let mut groups: Vec<(String, Vec<&DigestActionItem>)> = Vec::new();
        for a in &r.action_items {
            let owner = a
                .owner
                .as_deref()
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .unwrap_or("Unassigned")
                .to_string();
            if let Some(g) = groups.iter_mut().find(|(o, _)| *o == owner) {
                g.1.push(a);
            } else {
                groups.push((owner, vec![a]));
            }
        }
        for (owner, items) in &groups {
            s.push_str(&format!("**{owner}**\n\n"));
            for a in items {
                let src = source_suffix(&a.meeting, &a.date);
                if src.is_empty() {
                    s.push_str(&format!("- {}\n", a.title));
                } else {
                    s.push_str(&format!("- {} — {}\n", a.title, src));
                }
            }
            s.push('\n');
        }
    }
    if !r.trends.is_empty() {
        s.push_str("## Trends\n\n");
        for t in &r.trends {
            s.push_str(&format!("- {t}\n"));
        }
        s.push('\n');
    }
    if !r.open_items.is_empty() {
        s.push_str("## Open Items\n\n");
        for o in &r.open_items {
            s.push_str(&format!("- {o}\n"));
        }
        s.push('\n');
    }
    if !r.per_meeting.is_empty() {
        s.push_str("## Meetings\n\n");
        for m in &r.per_meeting {
            if m.date.trim().is_empty() {
                s.push_str(&format!("### {}\n\n", m.title));
            } else {
                s.push_str(&format!("### {} ({})\n\n", m.title, m.date));
            }
            if !m.recap.is_empty() {
                s.push_str(m.recap.trim());
                s.push_str("\n\n");
            } else if !m.has_intelligence {
                s.push_str("Not analyzed.\n\n");
            }
        }
    }
    s.trim_end().to_string()
}

/// Read a meeting's transcript text from its folder (final preferred).
fn read_meeting_transcript_text(dir: &Path) -> Option<String> {
    for name in [
        "meeting.transcript.final.txt",
        "meeting.transcript.txt",
        "mic.transcript.txt",
    ] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Pool transcript excerpts across meetings within `budget` chars. Each meeting
/// is allocated a relevance-weighted slice (floored so every meeting is
/// represented), then excerpted via the single-meeting retrieval-lite assembler,
/// and grouped under a `## label` header for citations.
fn assemble_multi_meeting_context(
    meetings: &[(String, String)],
    question: &str,
    budget: usize,
) -> String {
    let live: Vec<&(String, String)> = meetings
        .iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .collect();
    if live.is_empty() {
        return String::new();
    }
    if live.len() == 1 {
        let (label, text) = live[0];
        let excerpt = assemble_chat_transcript_context(text, question, budget);
        return format!("## {label}\n{excerpt}");
    }
    let n = live.len();
    let q_terms = chat_tokenize(question);
    // Relevance weight: distinct query terms present in a sample of each transcript.
    let weights: Vec<usize> = live
        .iter()
        .map(|(_, text)| {
            let sample: String = text.chars().take(16_000).collect();
            let toks = chat_tokenize(&sample);
            q_terms
                .iter()
                .filter(|qt| toks.iter().any(|tk| tk == *qt))
                .count()
        })
        .collect();
    let total_w: usize = weights.iter().sum();
    let floor = (budget / (n * 3)).max(800);
    let remainder = budget.saturating_sub(floor * n);
    let mut out = String::new();
    for (idx, (label, text)) in live.iter().enumerate() {
        // Relevance-weighted share of the remaining budget; equal split when no
        // query terms matched any meeting (total_w == 0) or on overflow.
        let extra = remainder
            .checked_mul(weights[idx])
            .and_then(|num| num.checked_div(total_w))
            .unwrap_or(remainder / n);
        let excerpt = assemble_chat_transcript_context(text, question, floor + extra);
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## {label}\n{excerpt}"));
    }
    out
}

/// One synthesis pass: draft → optional verify → parse JSON into a payload.
fn synthesize_digest_payload(
    input_text: &str,
    config: &MeetingCleanupConfig,
) -> Result<(DigestPayload, MeetingLlmCompletion), String> {
    let completion = complete_meeting_llm(
        MEETING_DIGEST_SYSTEM_PROMPT,
        input_text,
        config.clone(),
        meeting_ai_timeout(),
        meeting_ai_max_tokens(),
    )?;
    let completion = if meeting_ai_verification_enabled() {
        let draft = completion.content.clone();
        let verify_prompt = format!(
            "Source material:\n<<<MATERIAL\n{input_text}\nMATERIAL>>>\n\nDraft JSON:\n<<<JSON\n{draft}\nJSON>>>"
        );
        match complete_meeting_llm(
            MEETING_DIGEST_VERIFIER_SYSTEM_PROMPT,
            &verify_prompt,
            config.clone(),
            meeting_ai_timeout(),
            meeting_ai_max_tokens(),
        ) {
            Ok(v) => MeetingLlmCompletion {
                content: v.content,
                provider: v.provider,
                model: v.model,
                latency_ms: completion.latency_ms.saturating_add(v.latency_ms),
            },
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] digest verify failed; using draft");
                completion
            }
        }
    } else {
        completion
    };
    let json = extract_json_object(&completion.content)
        .ok_or_else(|| "digest synthesis returned no JSON object".to_string())?;
    let payload: DigestPayload =
        serde_json::from_str(&json).map_err(|e| format!("digest JSON parse failed: {e}"))?;
    Ok((payload, completion))
}

/// Blocking digest build: gather local per-meeting material, synthesize (with
/// map-reduce for large selections), and assemble the result + Markdown.
fn run_meeting_digest(refs: Vec<DigestMeetingRef>, missing: &str) -> Result<DigestResult, String> {
    let generate_missing = missing.eq_ignore_ascii_case("generate");
    let mut cards: Vec<DigestCard> = Vec::new();
    let mut skipped: Vec<DigestSkipped> = Vec::new();
    let mut per_meeting: Vec<DigestPerMeeting> = Vec::new();

    for r in &refs {
        let fallback_title = if r.title.trim().is_empty() {
            "Untitled meeting".to_string()
        } else {
            r.title.trim().to_string()
        };
        let dir = match meeting_dir_for_id(&r.id) {
            Ok(dir) => dir,
            Err(e) => {
                skipped.push(DigestSkipped {
                    id: r.id.clone(),
                    title: fallback_title.clone(),
                    reason: format!("invalid meeting id: {e}"),
                });
                per_meeting.push(DigestPerMeeting {
                    id: r.id.clone(),
                    title: fallback_title,
                    date: r.date.clone(),
                    recap: String::new(),
                    has_intelligence: false,
                });
                continue;
            }
        };
        let mut intel = load_cached_meeting_intelligence_from_dir(&dir)
            .ok()
            .flatten();
        if intel.is_none() && generate_missing {
            match read_meeting_transcript_text(&dir) {
                Some(text) => {
                    match run_meeting_intelligence(
                        MeetingAiTranscript {
                            source: "cached-final".to_string(),
                            text,
                        },
                        Some(dir.clone()),
                    ) {
                        Ok(generated) => intel = Some(generated),
                        Err(e) => skipped.push(DigestSkipped {
                            id: r.id.clone(),
                            title: fallback_title.clone(),
                            reason: format!("analysis failed: {e}"),
                        }),
                    }
                }
                None => skipped.push(DigestSkipped {
                    id: r.id.clone(),
                    title: fallback_title.clone(),
                    reason: "no transcript on this device to analyze".to_string(),
                }),
            }
        }
        match intel {
            Some(intel) => {
                let title = if !r.title.trim().is_empty() {
                    r.title.trim().to_string()
                } else if !intel.title.trim().is_empty() {
                    intel.title.trim().to_string()
                } else {
                    "Untitled meeting".to_string()
                };
                per_meeting.push(DigestPerMeeting {
                    id: r.id.clone(),
                    title: title.clone(),
                    date: r.date.clone(),
                    recap: digest_meeting_recap(&intel.summary, 280),
                    has_intelligence: true,
                });
                cards.push(DigestCard {
                    id: r.id.clone(),
                    title,
                    date: r.date.clone(),
                    summary: intel.summary,
                    decisions: intel.decisions.into_iter().map(|d| d.text).collect(),
                    actions: intel
                        .action_items
                        .into_iter()
                        .map(|a| (a.title, a.assignee))
                        .collect(),
                });
            }
            None => {
                if !generate_missing {
                    skipped.push(DigestSkipped {
                        id: r.id.clone(),
                        title: fallback_title.clone(),
                        reason: "not analyzed yet".to_string(),
                    });
                }
                per_meeting.push(DigestPerMeeting {
                    id: r.id.clone(),
                    title: fallback_title,
                    date: r.date.clone(),
                    recap: String::new(),
                    has_intelligence: false,
                });
            }
        }
    }

    if cards.is_empty() {
        return Err("None of the selected meetings have a summary on this device yet. Analyze at least one meeting (or choose \"Generate missing\") and try again.".to_string());
    }

    let summary_cap = env_u64(
        "AIRNOTE_MEETING_DIGEST_SUMMARY_CAP",
        DEFAULT_DIGEST_MEETING_SUMMARY_CAP as u64,
    ) as usize;
    let budget = env_u64(
        "AIRNOTE_MEETING_DIGEST_INPUT_CHAR_BUDGET",
        DEFAULT_MEETING_DIGEST_INPUT_CHAR_BUDGET as u64,
    ) as usize;
    let blocks: Vec<String> = cards
        .iter()
        .map(|c| build_meeting_block(c, summary_cap))
        .collect();
    let lens: Vec<usize> = blocks.iter().map(String::len).collect();
    let batches = pack_into_batches(&lens, budget);

    let config = meeting_ai_config()?;
    let (payload, provider, model, latency_ms) = if batches.len() <= 1 {
        let input = blocks.join("\n\n");
        let (payload, completion) = synthesize_digest_payload(&input, &config)?;
        (
            payload,
            completion.provider,
            completion.model,
            completion.latency_ms,
        )
    } else {
        // Map-reduce: summarize each batch, then merge the partial digests.
        let mut partials: Vec<String> = Vec::new();
        let mut total_latency = 0u64;
        for batch in &batches {
            let input = batch
                .iter()
                .map(|&i| blocks[i].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let (payload, completion) = synthesize_digest_payload(&input, &config)?;
            total_latency = total_latency.saturating_add(completion.latency_ms);
            partials.push(render_partial_digest(&payload));
        }
        let (payload, completion) = synthesize_digest_payload(&partials.join("\n\n"), &config)?;
        (
            payload,
            completion.provider,
            completion.model,
            total_latency.saturating_add(completion.latency_ms),
        )
    };

    let included_meeting_ids: Vec<String> = cards.iter().map(|c| c.id.clone()).collect();
    let title = nonempty_trimmed(payload.title.unwrap_or_default())
        .map(|t| normalize_meeting_title(&t))
        .unwrap_or_else(|| "Meeting Digest".to_string());
    let themes: Vec<DigestTheme> = payload
        .themes
        .into_iter()
        .filter_map(|t| {
            let title = nonempty_trimmed(t.title.unwrap_or_default())?;
            Some(DigestTheme {
                title,
                detail: t.detail.unwrap_or_default().trim().to_string(),
                meetings: t
                    .meetings
                    .into_iter()
                    .filter_map(nonempty_trimmed)
                    .collect(),
            })
        })
        .collect();
    let decisions: Vec<DigestDecisionItem> = payload
        .decisions
        .into_iter()
        .filter_map(|d| {
            let text = nonempty_trimmed(d.text.unwrap_or_default())?;
            Some(DigestDecisionItem {
                text,
                meeting: d.meeting.unwrap_or_default().trim().to_string(),
                date: d.date.unwrap_or_default().trim().to_string(),
            })
        })
        .collect();
    let action_items: Vec<DigestActionItem> = payload
        .action_items
        .into_iter()
        .filter_map(|a| {
            let title = nonempty_trimmed(a.title.unwrap_or_default())?;
            Some(DigestActionItem {
                title,
                owner: a.owner.and_then(nonempty_trimmed),
                meeting: a.meeting.unwrap_or_default().trim().to_string(),
                date: a.date.unwrap_or_default().trim().to_string(),
            })
        })
        .collect();
    let trends: Vec<String> = payload
        .trends
        .into_iter()
        .filter_map(nonempty_trimmed)
        .collect();
    let open_items: Vec<String> = payload
        .open_items
        .into_iter()
        .filter_map(nonempty_trimmed)
        .collect();

    let mut result = DigestResult {
        id: digest_cache_key(&included_meeting_ids, missing),
        title,
        date_range: digest_date_range(&refs),
        meeting_count: cards.len(),
        included_meeting_ids,
        skipped,
        executive_summary: payload
            .executive_summary
            .unwrap_or_default()
            .trim()
            .to_string(),
        themes,
        decisions,
        action_items,
        trends,
        open_items,
        per_meeting,
        markdown: String::new(),
        provider,
        model,
        latency_ms,
        created_at: now_ms(),
    };
    result.markdown = render_digest_markdown(&result);
    save_digest_to_history(&result);
    Ok(result)
}

/// Blocking digest chat: load each meeting's summary + transcript, assemble a
/// layered context (digest + per-meeting summaries + pooled transcript
/// excerpts), and stream the answer.
fn run_digest_chat(
    refs: Vec<DigestMeetingRef>,
    question: &str,
    digest_summary: Option<&str>,
    on_delta: impl FnMut(&str),
) -> Result<MeetingChatResult, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("question is empty".to_string());
    }
    let mut summaries: Vec<(String, String)> = Vec::new();
    let mut transcripts: Vec<(String, String)> = Vec::new();
    for r in &refs {
        let Ok(dir) = meeting_dir_for_id(&r.id) else {
            continue;
        };
        let label = digest_meeting_label(r);
        if let Ok(Some(intel)) = load_cached_meeting_intelligence_from_dir(&dir) {
            let summary = intel.summary.trim().to_string();
            if !summary.is_empty() {
                summaries.push((label.clone(), summary));
            }
        }
        if let Some(text) = read_meeting_transcript_text(&dir) {
            transcripts.push((label, text));
        }
    }
    if summaries.is_empty() && transcripts.is_empty() {
        return Err("None of the selected meetings have a local transcript or summary to chat about on this device.".to_string());
    }

    let config = meeting_ai_config()?;
    let budget = env_u64(
        "AIRNOTE_MEETING_DIGEST_CHAT_CHAR_BUDGET",
        DEFAULT_MEETING_DIGEST_CHAT_CHAR_BUDGET as u64,
    ) as usize;
    let transcript_context = assemble_multi_meeting_context(&transcripts, question, budget);

    let mut intel_block = String::new();
    if let Some(digest) = digest_summary.map(str::trim).filter(|d| !d.is_empty()) {
        intel_block.push_str("Cross-meeting digest summary:\n");
        intel_block.push_str(digest);
        intel_block.push_str("\n\n");
    }
    if !summaries.is_empty() {
        intel_block.push_str("Per-meeting summaries:\n");
        for (label, summary) in &summaries {
            intel_block.push_str(&format!(
                "## {label}\n{}\n\n",
                truncate_chars(summary, 1_800)
            ));
        }
    }
    let transcript_section = if transcript_context.trim().is_empty() {
        "(No transcript excerpts available; rely on the summaries above.)".to_string()
    } else {
        transcript_context
    };
    let labels = refs
        .iter()
        .map(digest_meeting_label)
        .collect::<Vec<_>>()
        .join("; ");
    let user_prompt = format!(
        "Selected meetings: {}\n\nMeeting intelligence:\n<<<INTEL\n{}\nINTEL>>>\n\nTranscript excerpts (most relevant to the question; \"## meeting\" groups, [mm:ss] timestamps, [...] = omitted spans):\n<<<TRANSCRIPTS\n{}\nTRANSCRIPTS>>>\n\nQuestion:\n{}",
        labels,
        intel_block.trim(),
        transcript_section,
        question
    );
    let completion = complete_meeting_llm_streaming(
        MEETING_DIGEST_CHAT_SYSTEM_PROMPT,
        &user_prompt,
        config,
        meeting_chat_timeout(),
        meeting_ai_max_tokens(),
        on_delta,
    )
    .inspect_err(|e| {
        tracing::warn!(error = %e, "[meeting_engine] digest chat request failed");
    })?;
    let answer = strip_llm_code_fences(&completion.content);
    if answer.trim().is_empty() {
        return Err("digest chat returned an empty answer".to_string());
    }
    Ok(MeetingChatResult {
        status: "completed".to_string(),
        provider: completion.provider,
        model: completion.model,
        latency_ms: completion.latency_ms,
        transcript_source: format!("{} meetings", refs.len()),
        answer,
    })
}

fn digests_dir() -> PathBuf {
    said_core::paths::data_dir()
        .join("meetings")
        .join(".digests")
}

/// Validate a digest id before using it in a path (prevents traversal).
fn safe_digest_id(id: &str) -> bool {
    id.starts_with("digest-")
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Persist a generated digest so it survives tab switches and app restarts.
/// Keyed by the digest id (same selection+strategy overwrites in place).
fn save_digest_to_history(result: &DigestResult) {
    if !safe_digest_id(&result.id) {
        return;
    }
    let dir = digests_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", result.id));
    match serde_json::to_vec_pretty(result) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(&path, bytes) {
                tracing::warn!(error = %e, "[meeting_engine] failed to save digest history");
            }
        }
        Err(e) => tracing::warn!(error = %e, "[meeting_engine] failed to serialize digest"),
    }
}

/// Lightweight entry for the digest history panel.
#[derive(Clone, Debug, Serialize)]
pub struct DigestHistoryEntry {
    id: String,
    title: String,
    date_range: String,
    meeting_count: usize,
    created_at: u64,
}

/// List saved digests, most recent first.
#[tauri::command]
pub fn meeting_engine_list_digests() -> Vec<DigestHistoryEntry> {
    let mut out: Vec<DigestHistoryEntry> = Vec::new();
    let Ok(entries) = fs::read_dir(digests_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(d) = serde_json::from_slice::<DigestResult>(&bytes) {
                out.push(DigestHistoryEntry {
                    id: d.id,
                    title: d.title,
                    date_range: d.date_range,
                    meeting_count: d.meeting_count,
                    created_at: d.created_at,
                });
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    out
}

/// Load a saved digest by id.
#[tauri::command]
pub fn meeting_engine_get_digest(id: String) -> Option<DigestResult> {
    if !safe_digest_id(&id) {
        return None;
    }
    let path = digests_dir().join(format!("{id}.json"));
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Delete a saved digest from history.
#[tauri::command]
pub fn meeting_engine_delete_digest(id: String) -> bool {
    if !safe_digest_id(&id) {
        return false;
    }
    fs::remove_file(digests_dir().join(format!("{id}.json"))).is_ok()
}

/// Synthesize a combined digest across the selected meetings.
#[tauri::command]
pub async fn meeting_engine_generate_digest(
    refs: Vec<DigestMeetingRef>,
    missing: Option<String>,
) -> Result<DigestResult, String> {
    if refs.is_empty() {
        return Err("Select at least one meeting to build a digest.".to_string());
    }
    let missing = missing.unwrap_or_else(|| "skip".to_string());
    tauri::async_runtime::spawn_blocking(move || run_meeting_digest(refs, &missing))
        .await
        .map_err(|e| format!("digest task failed: {e}"))?
}

/// Chat across the selected meetings, streaming the answer token-by-token.
#[tauri::command]
pub async fn meeting_engine_digest_chat(
    app: AppHandle,
    request_id: String,
    question: String,
    refs: Vec<DigestMeetingRef>,
    digest_summary: Option<String>,
) -> Result<MeetingChatResult, String> {
    if refs.is_empty() {
        return Err("No meetings selected for this chat.".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        run_digest_chat(refs, &question, digest_summary.as_deref(), |delta| {
            emit_main(
                &app,
                MEETING_CHAT_DELTA_EVENT,
                MeetingChatDelta {
                    request_id: request_id.clone(),
                    delta: delta.to_string(),
                },
            );
        })
    })
    .await
    .map_err(|e| format!("digest chat task failed: {e}"))?
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    fn rf(id: &str, title: &str, date: &str) -> DigestMeetingRef {
        DigestMeetingRef {
            id: id.to_string(),
            title: title.to_string(),
            date: date.to_string(),
        }
    }

    #[test]
    fn truncate_chars_keeps_short_and_cuts_long() {
        assert_eq!(truncate_chars("  hi  ", 10), "hi");
        let long = "a".repeat(50);
        let out = truncate_chars(&long, 10);
        assert_eq!(out.chars().count(), 12); // 10 + " …"
        assert!(out.ends_with(" …"));
        // Multi-byte safe (no panic on char boundary).
        let unicode = "नमस्ते दुनिया यह एक परीक्षण है".repeat(3);
        let _ = truncate_chars(&unicode, 5);
    }

    #[test]
    fn pack_into_batches_splits_on_budget() {
        assert!(pack_into_batches(&[], 100).is_empty());
        assert_eq!(pack_into_batches(&[10, 20, 30], 100), vec![vec![0, 1, 2]]);
        assert_eq!(
            pack_into_batches(&[60, 60, 60], 100),
            vec![vec![0], vec![1], vec![2]]
        );
        assert_eq!(
            pack_into_batches(&[40, 40, 40, 40], 100),
            vec![vec![0, 1], vec![2, 3]]
        );
        // A single oversized block still gets its own batch.
        assert_eq!(pack_into_batches(&[500], 100), vec![vec![0]]);
    }

    #[test]
    fn build_meeting_block_caps_summary_and_lists_items() {
        let card = DigestCard {
            id: "m1".into(),
            title: "Pricing Sync".into(),
            date: "16 Jun 2026".into(),
            summary: "x".repeat(20_000),
            decisions: vec!["Ship per-seat pricing".into(), "  ".into()],
            actions: vec![
                ("Draft the deck".into(), Some("Abhishek".into())),
                ("Email finance".into(), None),
            ],
        };
        let block = build_meeting_block(&card, 100);
        assert!(block.contains("### Pricing Sync (16 Jun 2026)"));
        assert!(block.contains("Summary:"));
        assert!(block.contains(" …")); // summary truncated
        assert!(block.contains("- Ship per-seat pricing"));
        assert!(block.contains("- Abhishek — Draft the deck"));
        assert!(block.contains("- Email finance"));
        assert!(!block.contains("-  \n")); // blank decision dropped
    }

    #[test]
    fn date_range_handles_same_and_span_and_empty() {
        assert_eq!(
            digest_date_range(&[rf("a", "A", "16 Jun 2026"), rf("b", "B", "16 Jun 2026")]),
            "16 Jun 2026"
        );
        assert_eq!(
            digest_date_range(&[rf("a", "A", "10 Jun 2026"), rf("b", "B", "16 Jun 2026")]),
            "10 Jun 2026 – 16 Jun 2026"
        );
        assert_eq!(digest_date_range(&[rf("a", "A", "")]), "");
    }

    #[test]
    fn cache_key_is_order_independent_and_strategy_sensitive() {
        let a = digest_cache_key(&["m2".into(), "m1".into()], "skip");
        let b = digest_cache_key(&["m1".into(), "m2".into()], "skip");
        assert_eq!(a, b, "key must not depend on id order");
        let c = digest_cache_key(&["m1".into(), "m2".into()], "generate");
        assert_ne!(b, c, "strategy must change the key");
    }

    #[test]
    fn multi_meeting_context_represents_every_meeting() {
        let meetings = vec![
            (
                "Alpha · d1".to_string(),
                "[00:01] we discussed the banana supply chain\n[00:02] and shipping costs"
                    .to_string(),
            ),
            (
                "Beta · d2".to_string(),
                "[00:01] the cat sat\n[00:02] on the mat".to_string(),
            ),
        ];
        let ctx = assemble_multi_meeting_context(&meetings, "banana", 10_000);
        assert!(ctx.contains("## Alpha · d1"));
        assert!(ctx.contains("## Beta · d2"));
        assert!(ctx.contains("banana"));
        // Empty transcripts are skipped entirely.
        let with_empty = vec![
            ("Gamma".to_string(), "   ".to_string()),
            ("Delta".to_string(), "[00:01] hello world".to_string()),
        ];
        let ctx2 = assemble_multi_meeting_context(&with_empty, "hello", 10_000);
        assert!(!ctx2.contains("## Gamma"));
        assert!(ctx2.contains("## Delta"));
    }

    #[test]
    fn render_markdown_has_sections_and_groups_actions_by_owner() {
        let result = DigestResult {
            id: "digest-x".into(),
            title: "Sentinel Week".into(),
            date_range: "10 Jun – 16 Jun 2026".into(),
            meeting_count: 2,
            included_meeting_ids: vec!["m1".into(), "m2".into()],
            skipped: vec![DigestSkipped {
                id: "m3".into(),
                title: "Untitled".into(),
                reason: "not analyzed yet".into(),
            }],
            executive_summary: "We aligned on pricing and rollout.".into(),
            themes: vec![DigestTheme {
                title: "Pricing".into(),
                detail: "Discussed across two meetings.".into(),
                meetings: vec!["Kickoff".into(), "Review".into()],
            }],
            decisions: vec![DigestDecisionItem {
                text: "Ship per-seat".into(),
                meeting: "Kickoff".into(),
                date: "10 Jun 2026".into(),
            }],
            action_items: vec![
                DigestActionItem {
                    title: "Draft deck".into(),
                    owner: Some("Abhishek".into()),
                    meeting: "Kickoff".into(),
                    date: String::new(),
                },
                DigestActionItem {
                    title: "Send invite".into(),
                    owner: Some("Abhishek".into()),
                    meeting: "Review".into(),
                    date: String::new(),
                },
                DigestActionItem {
                    title: "Book room".into(),
                    owner: None,
                    meeting: "Review".into(),
                    date: String::new(),
                },
            ],
            trends: vec!["Scope narrowed".into()],
            open_items: vec!["Confirm legal".into()],
            per_meeting: vec![DigestPerMeeting {
                id: "m1".into(),
                title: "Kickoff".into(),
                date: "10 Jun 2026".into(),
                recap: "Set the agenda.".into(),
                has_intelligence: true,
            }],
            markdown: String::new(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            latency_ms: 1,
            created_at: 0,
        };
        let md = render_digest_markdown(&result);
        assert!(md.starts_with("# Sentinel Week"));
        assert!(md.contains("10 Jun – 16 Jun 2026 · 2 meetings"));
        assert!(md.contains("1 meeting skipped (not analyzed)."));
        assert!(md.contains("## Executive Summary"));
        assert!(md.contains("## Key Themes"));
        assert!(md.contains("Meetings: Kickoff, Review"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("- Ship per-seat — Kickoff, 10 Jun 2026"));
        assert!(md.contains("## Action Items"));
        assert!(md.contains("**Abhishek**"));
        assert!(md.contains("**Unassigned**"));
        // Abhishek's two items grouped under one heading.
        assert_eq!(md.matches("**Abhishek**").count(), 1);
        assert!(md.contains("## Meetings"));
        assert!(md.contains("### Kickoff (10 Jun 2026)"));
        assert!(!md.contains('_')); // no italics that Lark would render literally
    }
}

#[derive(Debug, Deserialize)]
struct MeetingIntelligencePayload {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    summary: Option<String>,
    #[serde(default)]
    action_items: Vec<MeetingActionItemPayload>,
    #[serde(default)]
    decisions: Vec<MeetingDecisionPayload>,
}

#[derive(Debug, Deserialize)]
struct MeetingActionItemPayload {
    title: Option<String>,
    assignee: Option<String>,
    due: Option<String>,
    evidence: Option<String>,
    support: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MeetingDecisionPayload {
    Text(String),
    Object {
        text: Option<String>,
        evidence: Option<String>,
        support: Option<String>,
    },
}

type ParsedMeetingIntelligence = (
    String,
    Vec<String>,
    String,
    Vec<MeetingAiActionItem>,
    Vec<MeetingAiDecision>,
);

fn parse_meeting_intelligence(
    content: &str,
    transcript: Option<&str>,
) -> Result<ParsedMeetingIntelligence, String> {
    let json_text = extract_json_object(content)
        .ok_or_else(|| "meeting intelligence returned no JSON object".to_string())?;
    let payload: MeetingIntelligencePayload = serde_json::from_str(&json_text)
        .map_err(|e| format!("meeting intelligence JSON parse failed: {e}"))?;
    let title = payload
        .title
        .and_then(nonempty_trimmed)
        .map(|title| normalize_meeting_title(&title))
        .unwrap_or_default();
    let tags = normalize_meeting_tags(payload.tags);
    let summary = payload.summary.unwrap_or_default().trim().to_string();
    let action_items = payload
        .action_items
        .into_iter()
        .filter_map(|item| {
            let title = item.title?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            let evidence = item.evidence.and_then(nonempty_trimmed);
            if !meeting_ai_support_allowed(transcript, item.support.as_deref(), "firm") {
                return None;
            }
            if !meeting_ai_evidence_allowed(transcript, evidence.as_deref()) {
                return None;
            }
            Some(MeetingAiActionItem {
                title,
                assignee: item.assignee.and_then(nonempty_trimmed),
                due: item.due.and_then(nonempty_trimmed),
                evidence,
            })
        })
        .collect();
    let decisions = payload
        .decisions
        .into_iter()
        .filter_map(|decision| {
            let (text, evidence, support) = match decision {
                MeetingDecisionPayload::Text(text) => (nonempty_trimmed(text), None, None),
                MeetingDecisionPayload::Object {
                    text,
                    evidence,
                    support,
                } => (
                    text.and_then(nonempty_trimmed),
                    evidence.and_then(nonempty_trimmed),
                    support,
                ),
            };
            let text = text?;
            if !meeting_ai_support_allowed(transcript, support.as_deref(), "explicit") {
                return None;
            }
            if !meeting_ai_evidence_allowed(transcript, evidence.as_deref()) {
                return None;
            }
            Some(MeetingAiDecision { text, evidence })
        })
        .collect();
    Ok((title, tags, summary, action_items, decisions))
}

/// Clamp an AI-generated title to a clean single line (no surrounding quotes,
/// no trailing period, bounded length) suitable for a meeting heading.
fn normalize_meeting_title(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches('.')
        .trim();
    let mut title: String = cleaned.chars().take(80).collect();
    if title.len() < cleaned.len() {
        title = title.trim_end().to_string();
    }
    title
}

/// Normalize AI tags: trim, strip a leading `#`, drop empties, de-duplicate
/// case-insensitively, and cap to a sensible count.
fn normalize_meeting_tags<I: IntoIterator<Item = String>>(tags: I) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for tag in tags {
        let tag = tag.trim().trim_start_matches('#').trim();
        if tag.is_empty() || tag.len() > 32 {
            continue;
        }
        if seen.insert(tag.to_lowercase()) {
            out.push(tag.to_string());
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

fn meeting_ai_support_allowed(
    transcript: Option<&str>,
    support: Option<&str>,
    expected: &str,
) -> bool {
    if transcript.is_none() {
        return true;
    }

    support
        .map(|support| support.trim().eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn meeting_ai_evidence_allowed(transcript: Option<&str>, evidence: Option<&str>) -> bool {
    match transcript {
        Some(transcript) => evidence
            .map(|evidence| evidence_quote_matches_transcript(evidence, transcript))
            .unwrap_or(false),
        None => true,
    }
}

fn evidence_quote_matches_transcript(evidence: &str, transcript: &str) -> bool {
    let evidence = normalize_evidence_text(evidence);
    let evidence_tokens: Vec<&str> = evidence.split_whitespace().collect();
    if evidence_tokens.len() < 3 {
        return false;
    }

    let transcript = normalize_evidence_text(transcript);
    if transcript.contains(&evidence) {
        return true;
    }

    // Fallback: require a CONTIGUOUS phrase match, not scattered word overlap.
    // The old "60% of the quote's words appear somewhere" gate was nearly a
    // no-op on long transcripts — common words ("we", "the", "to", "is") almost
    // always appear, so fabricated quotes assembled from filler passed. A real
    // quote (even lightly paraphrased, or split across speaker/timestamp labels)
    // still shares a solid run of consecutive words; a fabrication rarely does.
    let transcript_tokens: Vec<&str> = transcript.split_whitespace().collect();
    let longest_run = longest_common_token_run(&evidence_tokens, &transcript_tokens);
    // Demand a 5-token contiguous phrase, or — for short quotes — most of the
    // quote as one run (never below 3).
    let required = 5
        .min(((evidence_tokens.len() as f32) * 0.6).ceil() as usize)
        .max(3);
    longest_run >= required
}

/// Length (in tokens) of the longest run of consecutive `needle` tokens that
/// appears as a contiguous slice of `haystack` — i.e. the longest common
/// contiguous token substring. O(needle·haystack) time, O(needle) space.
fn longest_common_token_run(needle: &[&str], haystack: &[&str]) -> usize {
    if needle.is_empty() || haystack.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; needle.len() + 1];
    let mut curr = vec![0usize; needle.len() + 1];
    let mut best = 0;
    for &h in haystack {
        for i in 0..needle.len() {
            curr[i + 1] = if needle[i] == h { prev[i] + 1 } else { 0 };
            best = best.max(curr[i + 1]);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    best
}

fn normalize_evidence_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '\'' {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn nonempty_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn extract_json_object(content: &str) -> Option<String> {
    let text = strip_llm_code_fences(content);
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(text[start..=end].to_string())
}

#[derive(Debug, Default, Deserialize)]
struct CleanupChatResponse {
    #[serde(default)]
    choices: Vec<CleanupChatChoice>,
}

#[derive(Debug, Default, Deserialize)]
struct CleanupChatChoice {
    #[serde(default)]
    message: CleanupChatMessage,
}

#[derive(Debug, Default, Deserialize)]
struct CleanupChatMessage {
    #[serde(default)]
    content: String,
}

/// One SSE `data:` frame from an OpenAI-compatible streaming chat completion.
#[derive(Debug, Deserialize)]
struct ChatStreamChunk {
    #[serde(default)]
    choices: Vec<ChatStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamChoice {
    #[serde(default)]
    delta: ChatStreamDelta,
}

#[derive(Debug, Default, Deserialize)]
struct ChatStreamDelta {
    #[serde(default)]
    content: Option<String>,
}

/// True when the cleaned transcript's word volume is within a sane band of the
/// raw (0.5×–2×). Outside that, the LLM rewrote/dropped content rather than
/// correcting it, and the cleanup should be rejected in favor of the raw text.
fn cleanup_within_volume_band(raw: &str, cleaned: &str) -> bool {
    let raw_words = raw.split_whitespace().count().max(1);
    let cleaned_words = cleaned.split_whitespace().count();
    let ratio = cleaned_words as f32 / raw_words as f32;
    (0.5..=2.0).contains(&ratio)
}

fn name_meeting_speakers_with_ai(
    segments: &mut [MeetingTranscriptSegment],
    cleaned_transcript: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let Some(user_prompt) = build_speaker_naming_prompt(segments, cleaned_transcript) else {
        return Ok(Vec::new());
    };
    let config = match meeting_ai_config() {
        Ok(config) => config,
        Err(e) => {
            tracing::debug!(
                error = %e,
                "[meeting_engine] speaker naming skipped because meeting AI is unavailable"
            );
            return Ok(Vec::new());
        }
    };
    let completion = complete_meeting_llm(
        MEETING_SPEAKER_NAMING_SYSTEM_PROMPT,
        &user_prompt,
        config,
        meeting_speaker_naming_timeout(),
        meeting_speaker_naming_max_tokens(),
    )?;
    let names = parse_speaker_naming_response(&completion.content, segments)?;
    Ok(apply_speaker_name_map(segments, &names))
}

fn build_speaker_naming_prompt(
    segments: &[MeetingTranscriptSegment],
    cleaned_transcript: Option<&str>,
) -> Option<String> {
    let mut seen_speakers = std::collections::HashSet::new();
    let speakers = segments
        .iter()
        .filter(|segment| seen_speakers.insert(segment.speaker_id.as_str()))
        .map(|segment| {
            format!(
                "- speaker_id={} current_label=\"{}\" source={}",
                segment.speaker_id, segment.speaker_name, segment.source
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if speakers.trim().is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    let mut total_chars = 0usize;
    for segment in segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
    {
        let line = format!(
            "[{} {} id={} source={}] {}",
            format_timestamp_ms(segment.start_ms),
            segment.speaker_name,
            segment.speaker_id,
            segment.source,
            truncate_chars(&compact_transcript_text(&segment.text), 260)
        );
        total_chars += line.len();
        if total_chars > 14_000 {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }

    let cleaned = cleaned_transcript
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            format!(
                "\n\nCleaned transcript excerpt, if useful:\n<<<CLEANED\n{}\nCLEANED>>>",
                truncate_chars(text, 6_000)
            )
        })
        .unwrap_or_default();

    Some(format!(
        "Existing speaker IDs:\n{speakers}\n\nTranscript excerpt:\n<<<TRANSCRIPT\n{}\nTRANSCRIPT>>>{cleaned}\n\nReturn only speaker_id/name pairs when the transcript gives direct evidence. Omit uncertain speakers.",
        lines.join("\n")
    ))
}

fn parse_speaker_naming_response(
    content: &str,
    segments: &[MeetingTranscriptSegment],
) -> Result<std::collections::HashMap<String, String>, String> {
    let json_text = extract_json_object(content)
        .ok_or_else(|| "speaker naming returned no JSON object".to_string())?;
    let payload: MeetingSpeakerNamingPayload = serde_json::from_str(&json_text)
        .map_err(|e| format!("speaker naming returned invalid JSON: {e}"))?;
    let known_ids = segments
        .iter()
        .map(|segment| segment.speaker_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut names = std::collections::HashMap::new();
    for item in payload.speakers {
        let speaker_id = item.speaker_id.trim();
        if !known_ids.contains(speaker_id) || item.evidence.trim().is_empty() {
            continue;
        }
        let Some(name) = sanitize_inferred_speaker_name(&item.name) else {
            continue;
        };
        names.insert(speaker_id.to_string(), name);
    }
    Ok(names)
}

fn apply_speaker_name_map(
    segments: &mut [MeetingTranscriptSegment],
    names: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut replacements: Vec<(String, String)> = Vec::new();
    for segment in segments {
        let Some(name) = names.get(&segment.speaker_id) else {
            continue;
        };
        if segment.speaker_name == *name {
            continue;
        }
        let old = segment.speaker_name.clone();
        if !replacements
            .iter()
            .any(|(existing_old, existing_new)| existing_old == &old && existing_new == name)
        {
            replacements.push((old, name.clone()));
        }
        segment.speaker_name = name.clone();
    }
    replacements
}

fn rewrite_speaker_labels_in_text(text: &str, replacements: &[(String, String)]) -> String {
    let mut rewritten = text.to_string();
    for (old, new) in replacements {
        let old = old.trim();
        let new = new.trim();
        if old.is_empty() || new.is_empty() || old == new {
            continue;
        }
        rewritten = rewritten.replace(&format!(" {old}]"), &format!(" {new}]"));
        rewritten = rewritten.replace(&format!("[{old}]"), &format!("[{new}]"));
        rewritten = rewritten.replace(&format!("{old}:"), &format!("{new}:"));
        rewritten = rewritten.replace(&format!("{old} -"), &format!("{new} -"));
    }
    rewritten
}

fn sanitize_inferred_speaker_name(name: &str) -> Option<String> {
    let cleaned = name
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '[' | ']' | '(' | ')'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() || cleaned.len() > 48 || cleaned.split_whitespace().count() > 4 {
        return None;
    }
    if cleaned.chars().any(|c| {
        !(c.is_alphabetic() || c.is_whitespace() || matches!(c, '.' | '\'' | '-' | '’' | '‘' | 'ʼ'))
    }) {
        return None;
    }
    if cleaned.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();
    let generic = [
        "you",
        "me",
        "i",
        "speaker",
        "unknown",
        "participant",
        "user",
        "host",
        "customer",
        "client",
        "agent",
        "assistant",
        "moderator",
        "presenter",
        "attendee",
        "person",
        "sir",
        "madam",
        "maam",
        "ma'am",
    ];
    if generic.contains(&lower.as_str()) || lower.starts_with("speaker ") {
        return None;
    }
    Some(cleaned)
}

fn compact_transcript_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cleanup_meeting_transcript_with_llm(
    raw: &str,
    config: MeetingCleanupConfig,
) -> Result<MeetingCleanupResult, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty transcript; meeting cleanup not run".to_string());
    }

    let timeout = Duration::from_secs(env_u64(
        "AIRNOTE_MEETING_CLEANUP_TIMEOUT_SECS",
        DEFAULT_MEETING_CLEANUP_TIMEOUT_SECS,
    ));
    let max_tokens = env_u64(
        "AIRNOTE_MEETING_CLEANUP_MAX_TOKENS",
        DEFAULT_MEETING_CLEANUP_MAX_TOKENS,
    );
    let user_prompt = format!(
        "The transcript below was captured by AirNote's local meeting recorder.\nClean it according to the system rules.\n\nRaw transcript:\n<<<TRANSCRIPT\n{}\nTRANSCRIPT>>>",
        raw
    );
    let completion = complete_meeting_llm(
        MEETING_CLEANUP_SYSTEM_PROMPT,
        &user_prompt,
        config,
        timeout,
        max_tokens,
    )?;
    let cleaned = strip_llm_transcript_wrappers(&completion.content);
    if cleaned.trim().is_empty() {
        return Err("meeting cleanup returned an empty transcript".to_string());
    }

    // Over-correction guard: a faithful cleanup keeps roughly the same volume of
    // words (it fixes ASR slips, it doesn't rewrite). If the cleaned text
    // collapsed or ballooned far outside a sane band, the model rewrote/dropped
    // content — reject it so the caller keeps the raw transcript rather than a
    // confidently-wrong one (temp-0 cleanup can "correct" real names into
    // plausible-but-wrong words).
    if !cleanup_within_volume_band(raw, &cleaned) {
        let raw_words = raw.split_whitespace().count();
        let cleaned_words = cleaned.split_whitespace().count();
        return Err(format!(
            "meeting cleanup changed transcript length too much ({raw_words} → {cleaned_words} words); rejecting as over-correction"
        ));
    }

    Ok(MeetingCleanupResult {
        transcript: cleaned,
        provider: completion.provider,
        model: completion.model,
        latency_ms: completion.latency_ms,
    })
}

/// Backoff before retrying a transient meeting-LLM failure. On HTTP 429 it
/// honors the provider's `Retry-After` (seconds, clamped) so a rate-limited
/// long meeting pauses briefly instead of hammering the same window and failing;
/// otherwise exponential backoff with a ceiling.
fn meeting_llm_retry_delay(status_code: u16, retry_after: Option<&str>, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(5);
    if status_code == 429 {
        if let Some(secs) = retry_after.and_then(|h| h.trim().parse::<u64>().ok()) {
            return Duration::from_secs(secs.clamp(1, 30));
        }
        return Duration::from_millis((2_000u64 << shift).min(30_000));
    }
    Duration::from_millis((800u64 << shift).min(8_000))
}

/// Human-readable, classified error for a non-2xx meeting-LLM response. The
/// wording is actionable (points at the API key / model) and also feeds the
/// downstream terminal-vs-retry classifier.
fn meeting_llm_status_error(provider: &str, status: u16, body: &str) -> String {
    let detail = truncate_error(body.trim());
    match status {
        401 | 403 => format!(
            "meeting AI authentication failed ({status}) for '{provider}' — the API key is missing, invalid, or expired. {detail}"
        ),
        429 => format!("meeting AI rate-limited ({status}) by '{provider}'. {detail}"),
        400 | 404 | 422 => format!(
            "meeting AI bad request ({status}) for '{provider}' — likely an unsupported model or parameter. {detail}"
        ),
        _ => format!("meeting AI provider error ({status}) from '{provider}': {detail}"),
    }
}

fn complete_meeting_llm(
    system_prompt: &str,
    user_prompt: &str,
    config: MeetingCleanupConfig,
    timeout: Duration,
    max_tokens: u64,
) -> Result<MeetingLlmCompletion, String> {
    let mut body = serde_json::json!({
        "model": &config.model,
        "stream": false,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_prompt }
        ]
    });
    if config.provider == "deepseek" {
        body["thinking"] = serde_json::json!({ "type": "disabled" });
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("meeting AI client failed: {e}"))?;

    // Retry transient failures (rate limits, 5xx, network/timeout) with backoff.
    // Non-streaming callers (cleanup, intelligence, transliteration, verifier)
    // are batch operations, so a brief retry turns the most common provider
    // hiccup into a non-event instead of a hard pipeline failure.
    const MEETING_LLM_MAX_ATTEMPTS: u32 = 3;
    let started = Instant::now();
    let mut attempt = 0;
    let response = loop {
        attempt += 1;
        let send_result = client
            .post(&config.url)
            .header(&config.auth_header_name, &config.auth_header_value)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let response = match send_result {
            Ok(response) => response,
            Err(e) => {
                if attempt < MEETING_LLM_MAX_ATTEMPTS {
                    tracing::warn!(attempt, error = %e, "[meeting_engine] LLM request errored; retrying");
                    thread::sleep(meeting_llm_retry_delay(0, None, attempt));
                    continue;
                }
                let hint = if e.is_timeout() {
                    " — provider timed out; increase AIRNOTE_MEETING_AI_TIMEOUT_SECS or switch provider"
                } else {
                    " — check network / provider availability"
                };
                return Err(format!(
                    "meeting AI request to '{}' failed: {e}{hint}",
                    config.provider
                ));
            }
        };
        let status = response.status();
        if status.is_success() {
            break response;
        }
        // Only transient classes are retried; auth (401/403) and bad-request
        // (400/404/422) fail fast with a clear message instead of looping.
        let retryable = status.as_u16() == 429 || status.is_server_error();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let status_code = status.as_u16();
        let body_text = response.text().unwrap_or_default();
        if retryable && attempt < MEETING_LLM_MAX_ATTEMPTS {
            let delay = meeting_llm_retry_delay(status_code, retry_after.as_deref(), attempt);
            tracing::warn!(attempt, %status, delay_ms = delay.as_millis() as u64, "[meeting_engine] LLM transient error; retrying");
            thread::sleep(delay);
            continue;
        }
        return Err(meeting_llm_status_error(
            &config.provider,
            status_code,
            &body_text,
        ));
    };

    let response: CleanupChatResponse = response.json().map_err(|e| {
        format!(
            "meeting AI response from '{}' was unreadable (malformed or truncated): {e}",
            config.provider
        )
    })?;
    let content = response
        .choices
        .first()
        .map(|choice| choice.message.content.as_str())
        .unwrap_or("");
    if content.trim().is_empty() {
        return Err(format!(
            "meeting AI ('{}') returned an empty response",
            config.provider
        ));
    }

    Ok(MeetingLlmCompletion {
        content: content.trim().to_string(),
        provider: config.provider,
        model: config.model,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// Streaming variant of [`complete_meeting_llm`]. Requests `stream: true` and
/// parses the OpenAI-compatible SSE response, invoking `on_delta` for each
/// content chunk as it arrives. Returns the fully accumulated completion. Used
/// by the chat command so the answer renders token-by-token instead of landing
/// all at once.
fn complete_meeting_llm_streaming(
    system_prompt: &str,
    user_prompt: &str,
    config: MeetingCleanupConfig,
    timeout: Duration,
    max_tokens: u64,
    mut on_delta: impl FnMut(&str),
) -> Result<MeetingLlmCompletion, String> {
    let mut body = serde_json::json!({
        "model": &config.model,
        "stream": true,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_prompt }
        ]
    });
    if config.provider == "deepseek" {
        body["thinking"] = serde_json::json!({ "type": "disabled" });
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("meeting AI client failed: {e}"))?;
    let started = Instant::now();
    // Retry the connect/handshake (before any tokens stream) on transient
    // failures, same policy as the non-streaming path. Once the 2xx stream
    // starts we don't retry — a mid-stream drop is handled below.
    const MEETING_LLM_MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0;
    let response = loop {
        attempt += 1;
        let send_result = client
            .post(&config.url)
            .header(&config.auth_header_name, &config.auth_header_value)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let response = match send_result {
            Ok(response) => response,
            Err(e) => {
                if attempt < MEETING_LLM_MAX_ATTEMPTS {
                    tracing::warn!(attempt, error = %e, "[meeting_engine] chat LLM request errored; retrying");
                    thread::sleep(meeting_llm_retry_delay(0, None, attempt));
                    continue;
                }
                let hint = if e.is_timeout() {
                    " — provider timed out; increase AIRNOTE_MEETING_CHAT_TIMEOUT_SECS or switch provider"
                } else {
                    " — check network / provider availability"
                };
                return Err(format!(
                    "meeting AI request to '{}' failed: {e}{hint}",
                    config.provider
                ));
            }
        };
        let status = response.status();
        if status.is_success() {
            break response;
        }
        let retryable = status.as_u16() == 429 || status.is_server_error();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let status_code = status.as_u16();
        let body_text = response.text().unwrap_or_default();
        if retryable && attempt < MEETING_LLM_MAX_ATTEMPTS {
            let delay = meeting_llm_retry_delay(status_code, retry_after.as_deref(), attempt);
            tracing::warn!(attempt, %status, delay_ms = delay.as_millis() as u64, "[meeting_engine] chat LLM transient error; retrying");
            thread::sleep(delay);
            continue;
        }
        return Err(meeting_llm_status_error(
            &config.provider,
            status_code,
            &body_text,
        ));
    };

    let mut reader = std::io::BufReader::new(response);
    let mut line = String::new();
    let mut content = String::new();
    let mut saw_done = false;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("meeting AI stream read failed: {e}"))?;
        if read == 0 {
            break;
        }
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            saw_done = true;
            break;
        }
        let Ok(chunk) = serde_json::from_str::<ChatStreamChunk>(data) else {
            continue;
        };
        if let Some(delta) = chunk
            .choices
            .first()
            .and_then(|choice| choice.delta.content.as_deref())
            .filter(|delta| !delta.is_empty())
        {
            content.push_str(delta);
            on_delta(delta);
        }
    }

    if content.trim().is_empty() {
        return Err(format!(
            "meeting AI ('{}') returned an empty response",
            config.provider
        ));
    }
    // Stream ended without the [DONE] sentinel — the connection dropped
    // mid-answer. Keep the partial text (better than nothing for chat) but flag
    // it so it's not silently treated as complete.
    if !saw_done {
        tracing::warn!(
            provider = %config.provider,
            "[meeting_engine] chat stream ended without [DONE]; answer may be truncated"
        );
    }

    Ok(MeetingLlmCompletion {
        content: content.trim().to_string(),
        provider: config.provider,
        model: config.model,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

fn meeting_cleanup_config() -> Result<MeetingCleanupConfig, String> {
    said_core::load_env();

    let provider = meeting_cleanup_provider();
    let model = meeting_cleanup_model(&provider);
    meeting_provider_config(provider, model, &["AIRNOTE_MEETING_CLEANUP_API_KEY"])
}

fn meeting_ai_config() -> Result<MeetingCleanupConfig, String> {
    said_core::load_env();

    // Meetings always use DeepSeek (no gateway/groq). Provider is locked; only
    // the model stays tunable via AIRNOTE_MEETING_AI_MODEL.
    let provider = meeting_cleanup_provider();
    let model = env_nonempty("AIRNOTE_MEETING_AI_MODEL")
        .unwrap_or_else(|| meeting_cleanup_model(&provider));
    meeting_provider_config(
        provider,
        model,
        &[
            "AIRNOTE_MEETING_AI_API_KEY",
            "AIRNOTE_MEETING_CLEANUP_API_KEY",
        ],
    )
}

/// Groq API key synced from Preferences (Settings → API keys) by the desktop.
/// Lets meeting AI use the user's saved key without a shell env var.
static RUNTIME_GROQ_API_KEY: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Called by the desktop whenever Preferences load/change. Empty/blank clears it.
pub fn set_runtime_groq_api_key(key: Option<String>) {
    let cleaned = key.and_then(|k| {
        let t = k.trim().to_string();
        (!t.is_empty()).then_some(t)
    });
    if let Ok(mut slot) = RUNTIME_GROQ_API_KEY.write() {
        *slot = cleaned;
    }
}

fn runtime_groq_api_key() -> Option<String> {
    RUNTIME_GROQ_API_KEY
        .read()
        .ok()
        .and_then(|slot| slot.clone())
}

/// DeepSeek API key baked into the build at compile time. DeepSeek is the
/// bundled meeting-summary provider and its key is fixed — users cannot change
/// it. Set `DEEPSEEK_API_KEY` in the build environment to bake it in (build-dmg.sh
/// exports it from `.env`). Returns None in dev builds where it wasn't baked, so
/// the caller falls back to a runtime env var.
fn bundled_deepseek_api_key() -> Option<String> {
    option_env!("DEEPSEEK_API_KEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

fn meeting_provider_config(
    provider: String,
    model: String,
    override_api_key_envs: &[&str],
) -> Result<MeetingCleanupConfig, String> {
    let override_key = override_api_key_envs
        .iter()
        .find_map(|name| env_nonempty(name));
    match provider.as_str() {
        "groq" => {
            // Priority: explicit per-meeting override env → key saved in Settings
            // (Preferences) → ambient GROQ_API_KEY shell env.
            let api_key = override_key
                .clone()
                .or_else(runtime_groq_api_key)
                .or_else(|| env_nonempty("GROQ_API_KEY"))
                .ok_or_else(|| {
                    "no Groq API key — set it in Settings → API keys (or GROQ_API_KEY)".to_string()
                })?;
            Ok(MeetingCleanupConfig {
                provider,
                url: GROQ_MEETING_CLEANUP_URL.to_string(),
                auth_header_name: "Authorization".to_string(),
                auth_header_value: format!("Bearer {api_key}"),
                model,
            })
        }
        "gateway" => {
            let api_key = override_key
                .clone()
                .or_else(|| env_nonempty("GATEWAY_API_KEY"))
                .or_else(|| {
                    let key = said_core::api_key();
                    if key.trim().is_empty() {
                        None
                    } else {
                        Some(key)
                    }
                })
                .ok_or_else(|| {
                    "no Gateway API key — set GATEWAY_API_KEY (gateway meeting AI provider)"
                        .to_string()
                })?;
            Ok(MeetingCleanupConfig {
                provider,
                url: GATEWAY_MEETING_CLEANUP_URL.to_string(),
                auth_header_name: "X-API-Key".to_string(),
                auth_header_value: api_key,
                model,
            })
        }
        "deepseek" => {
            // DeepSeek key is bundled into the build and fixed — not user-configurable.
            // Release builds bake it in via option_env! (DEEPSEEK_API_KEY set at build
            // time); dev builds fall back to a runtime DEEPSEEK_API_KEY env var.
            let api_key = bundled_deepseek_api_key()
                .or(override_key)
                .or_else(|| env_nonempty("DEEPSEEK_API_KEY"))
                .ok_or_else(|| {
                    // End-users can't fix this (the key is bundled at build time);
                    // keep it honest but actionable for whoever ships the build.
                    "meeting AI unavailable — no DeepSeek key in this build (bundle DEEPSEEK_API_KEY at build time, or set it as an env var)"
                        .to_string()
                })?;
            Ok(MeetingCleanupConfig {
                provider,
                url: DEEPSEEK_MEETING_CLEANUP_URL.to_string(),
                auth_header_name: "Authorization".to_string(),
                auth_header_value: format!("Bearer {api_key}"),
                model,
            })
        }
        other => Err(format!(
            "unsupported meeting cleanup provider '{other}'; use groq, gateway, or deepseek"
        )),
    }
}

fn meeting_ai_timeout() -> Duration {
    Duration::from_secs(env_u64(
        "AIRNOTE_MEETING_AI_TIMEOUT_SECS",
        DEFAULT_MEETING_AI_TIMEOUT_SECS,
    ))
}

fn meeting_speaker_naming_timeout() -> Duration {
    Duration::from_secs(env_u64(
        "AIRNOTE_MEETING_SPEAKER_NAMING_TIMEOUT_SECS",
        DEFAULT_MEETING_SPEAKER_NAMING_TIMEOUT_SECS,
    ))
}

/// Interactive chat gets a tighter timeout than batch cleanup/intelligence so a
/// stalled provider surfaces a clear error in ~a minute instead of leaving the
/// chat stuck on "thinking" for the full 2-minute batch timeout.
fn meeting_chat_timeout() -> Duration {
    Duration::from_secs(env_u64("AIRNOTE_MEETING_CHAT_TIMEOUT_SECS", 60))
}

fn meeting_ai_max_tokens() -> u64 {
    env_u64(
        "AIRNOTE_MEETING_AI_MAX_TOKENS",
        DEFAULT_MEETING_AI_MAX_TOKENS,
    )
}

fn meeting_speaker_naming_max_tokens() -> u64 {
    env_u64(
        "AIRNOTE_MEETING_SPEAKER_NAMING_MAX_TOKENS",
        DEFAULT_MEETING_SPEAKER_NAMING_MAX_TOKENS,
    )
}

fn meeting_ai_verification_enabled() -> bool {
    env_bool("AIRNOTE_MEETING_AI_VERIFY", true)
}

fn meeting_cleanup_provider() -> String {
    // Meetings always use DeepSeek for all AI (transcript cleanup + summary).
    // The provider is intentionally NOT env/user-switchable - no gateway/groq in
    // the meeting pipeline.
    DEFAULT_MEETING_CLEANUP_PROVIDER.to_string()
}

fn meeting_cleanup_model(provider: &str) -> String {
    env_nonempty("AIRNOTE_MEETING_CLEANUP_MODEL").unwrap_or_else(|| default_meeting_model(provider))
}

fn default_meeting_model(provider: &str) -> String {
    match provider {
        "groq" => DEFAULT_GROQ_MEETING_CLEANUP_MODEL.to_string(),
        "gateway" => DEFAULT_GATEWAY_MEETING_CLEANUP_MODEL.to_string(),
        "deepseek" => DEFAULT_DEEPSEEK_MEETING_CLEANUP_MODEL.to_string(),
        _ => DEFAULT_GROQ_MEETING_CLEANUP_MODEL.to_string(),
    }
}

fn meeting_final_diarization_enabled() -> bool {
    false
}

fn meeting_final_diarization_mode() -> String {
    FINAL_DIARIZATION_MODE_OFF.to_string()
}

fn normalize_meeting_final_diarization_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "light" | "light_onnx" | "onnx" => FINAL_DIARIZATION_MODE_LIGHT.to_string(),
        "high" | "sortformer" | "nemo" | "nemo_sortformer" | "external" => {
            FINAL_DIARIZATION_MODE_HIGH.to_string()
        }
        _ => FINAL_DIARIZATION_MODE_OFF.to_string(),
    }
}

fn meeting_final_diarization_runner() -> Result<Option<MeetingFinalDiarizationRunner>, String> {
    match meeting_final_diarization_mode().as_str() {
        FINAL_DIARIZATION_MODE_OFF => Ok(None),
        FINAL_DIARIZATION_MODE_LIGHT => {
            validate_light_diarization_models()?;
            Ok(Some(MeetingFinalDiarizationRunner::LightOnnx))
        }
        FINAL_DIARIZATION_MODE_HIGH => meeting_final_diarization_command_config()
            .map(|config| config.map(MeetingFinalDiarizationRunner::Command)),
        _ => Ok(None),
    }
}

fn meeting_final_diarization_command_config()
-> Result<Option<MeetingFinalDiarizationConfig>, String> {
    said_core::load_env();

    let timeout = Duration::from_secs(env_u64(
        "AIRNOTE_MEETING_FINAL_DIARIZATION_TIMEOUT_SECS",
        DEFAULT_FINAL_DIARIZATION_TIMEOUT_SECS,
    ));
    let provider = env_nonempty("AIRNOTE_MEETING_FINAL_DIARIZATION_PROVIDER")
        .unwrap_or_else(|| HIGH_DIARIZATION_PROVIDER.to_string());

    if let Some(script) = env_file_path("AIRNOTE_MEETING_FINAL_DIARIZATION_SCRIPT") {
        let command = env_executable_path("AIRNOTE_MEETING_FINAL_DIARIZATION_PYTHON")
            .or_else(|| find_on_path("python3"))
            .ok_or_else(|| {
                "python3 not found; set AIRNOTE_MEETING_FINAL_DIARIZATION_PYTHON".to_string()
            })?;
        return Ok(Some(MeetingFinalDiarizationConfig {
            provider,
            command,
            script: Some(script),
            timeout,
        }));
    }

    let has_script_env = env_nonempty("AIRNOTE_MEETING_FINAL_DIARIZATION_SCRIPT").is_some();
    if has_script_env {
        return Err(
            "AIRNOTE_MEETING_FINAL_DIARIZATION_SCRIPT is set but the file does not exist"
                .to_string(),
        );
    }

    if let Some(command) = env_executable_path("AIRNOTE_MEETING_FINAL_DIARIZATION_COMMAND") {
        return Ok(Some(MeetingFinalDiarizationConfig {
            provider,
            command,
            script: None,
            timeout,
        }));
    }

    if env_nonempty("AIRNOTE_MEETING_FINAL_DIARIZATION_COMMAND").is_some() {
        return Err(
            "AIRNOTE_MEETING_FINAL_DIARIZATION_COMMAND is set but is not executable or on PATH"
                .to_string(),
        );
    }

    Ok(None)
}

fn strip_llm_transcript_wrappers(content: &str) -> String {
    let mut text = strip_llm_code_fences(content);

    for prefix in [
        "Cleaned transcript:",
        "Cleaned Transcript:",
        "Transcript:",
        "Output:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
            break;
        }
    }

    text.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn strip_llm_code_fences(content: &str) -> String {
    let mut text = content.trim();
    if let Some(rest) = text.strip_prefix("```") {
        if let Some(first_line_end) = rest.find('\n') {
            text = &rest[first_line_end + 1..];
        } else {
            text = rest;
        }
        text = text.trim();
        if let Some(stripped) = text.strip_suffix("```") {
            text = stripped.trim();
        }
    }
    text.trim().to_string()
}

// ── Meeting settings store ────────────────────────────────────────────────────
// User-tweakable meeting settings (language, model, diarization, …) are persisted
// as a key→value map keyed by the same AIRNOTE_MEETING_* / AIRNOTE_WHISPER_* names
// the engine already reads. `meeting_env` overlays the store on top of the
// process env, so a UI setting wins over a shell env var which wins over the
// built-in default — and every existing `env_*` reader picks it up with no
// call-site rewrite. Changes apply to the next meeting (the engine reads these
// per job). Persisted atomically; a corrupt file is ignored, never crashes.

fn meeting_settings_path() -> PathBuf {
    said_core::paths::data_dir().join("meeting-settings.json")
}

fn meeting_settings_store() -> &'static RwLock<std::collections::BTreeMap<String, String>> {
    static STORE: OnceLock<RwLock<std::collections::BTreeMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| {
        let map = fs::read(meeting_settings_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        RwLock::new(map)
    })
}

/// Resolve a meeting setting: persisted UI value > shell env var > unset.
fn meeting_env(name: &str) -> Option<String> {
    if let Ok(store) = meeting_settings_store().read() {
        if let Some(value) = store.get(name) {
            if !value.trim().is_empty() {
                return Some(value.clone());
            }
        }
    }
    std::env::var(name).ok()
}

#[tauri::command]
pub fn meeting_settings_get() -> std::collections::BTreeMap<String, String> {
    meeting_settings_store()
        .read()
        .map(|store| store.clone())
        .unwrap_or_default()
}

/// Set (or clear, when `value` is None/empty) a single meeting setting and
/// persist atomically.
#[tauri::command]
pub fn meeting_settings_set(key: String, value: Option<String>) -> Result<(), String> {
    // Only allow our own namespaced keys — never let the UI write arbitrary env.
    if !(key.starts_with("AIRNOTE_MEETING_") || key.starts_with("AIRNOTE_WHISPER_")) {
        return Err(format!("rejected non-meeting setting key: {key}"));
    }
    let mut store = meeting_settings_store()
        .write()
        .map_err(|_| "meeting settings lock poisoned".to_string())?;
    match value {
        Some(v) if !v.trim().is_empty() => {
            store.insert(key, v.trim().to_string());
        }
        _ => {
            store.remove(&key);
        }
    }
    let bytes = serde_json::to_vec_pretty(&*store)
        .map_err(|e| format!("failed to serialize meeting settings: {e}"))?;
    write_atomic(meeting_settings_path(), bytes)
        .map_err(|e| format!("failed to write meeting settings: {e}"))
}

#[derive(Debug, Serialize)]
pub struct WhisperModelInfo {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub active: bool,
    /// Empty / partial / broken file — present but not usable for transcription.
    pub incomplete: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RemovedWhisperModelInfo {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WhisperModelCleanupResult {
    pub removed: Vec<RemovedWhisperModelInfo>,
    pub freed_bytes: u64,
}

/// List installed supported whisper.cpp meeting models in the app's model
/// directory, marking which one is currently active.
#[tauri::command]
pub fn meeting_list_whisper_models() -> Vec<WhisperModelInfo> {
    let dir = meeting_whisper_models_dir();
    let active = selected_whisper_model_path();
    let mut models = Vec::new();
    for (name, _, _) in WHISPER_MODEL_CATALOG {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        // fs::metadata follows symlinks (some models are symlinked to another
        // dir), so the size is the real target size, not the link's ~90 bytes.
        let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        models.push(WhisperModelInfo {
            name: (*name).to_string(),
            path: path.display().to_string(),
            size_bytes,
            active: active.as_deref() == Some(path.as_path()),
            incomplete: size_bytes < MIN_WHISPER_MODEL_BYTES,
        });
    }
    models
}

/// Reclaim disposable meeting storage on demand (orphan dirs + per-meeting
/// intermediates), reusing the startup GC.
#[tauri::command]
pub fn meeting_cleanup_storage() -> Result<(), String> {
    gc_orphan_meeting_dirs();
    Ok(())
}

// ── Whisper model download / management ───────────────────────────────────────

const MODEL_DOWNLOAD_EVENT: &str = "meeting-model-download";

/// Downloadable whisper.cpp models (name, source URL, approx size for a progress
/// estimate before Content-Length is known). These are shared by meeting
/// transcription and local Windows dictation; dictation uses the ACTIVE model
/// (`AIRNOTE_WHISPER_CPP_MODEL`). All are multilingual (no `*.en`) to preserve
/// Hindi/Hinglish. Sizes are HF download-size hints.
const WHISPER_MODEL_CATALOG: &[(&str, &str, u64)] = &[
    (
        "ggml-large-v3-turbo-q5_0.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        573_000_000,
    ),
    (
        "ggml-medium.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        1_530_000_000,
    ),
    (
        "ggml-small.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        488_000_000,
    ),
    (
        "ggml-base.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        148_000_000,
    ),
    (
        "ggml-tiny.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        78_000_000,
    ),
];

/// Silero VAD ggml model for whisper.cpp `--vad` (speech-only segments).
pub const SILERO_VAD_MODEL_NAME: &str = "ggml-silero-v5.1.2.bin";
const SILERO_VAD_MODEL_URL: &str =
    "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin";
const SILERO_VAD_SIZE_HINT: u64 = 900_000;
const MIN_SILERO_VAD_BYTES: u64 = 100_000;

fn meeting_whisper_models_dir() -> PathBuf {
    said_core::paths::data_dir().join("models")
}

fn model_download_cancels() -> &'static Mutex<std::collections::HashSet<String>> {
    static CANCELS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    CANCELS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn model_downloads_inflight() -> &'static Mutex<std::collections::HashSet<String>> {
    static INFLIGHT: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    INFLIGHT.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[derive(Debug, Clone, Serialize)]
struct ModelDownloadProgress {
    name: String,
    received: u64,
    total: u64,
    status: String, // "downloading" | "done" | "cancelled" | "error"
    error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CatalogModel {
    pub name: String,
    pub size_bytes: u64,
    pub installed: bool,
}

#[derive(Debug, Serialize)]
pub struct MeetingDiarizationSettingsStatus {
    pub mode: String,
    pub light_installed: bool,
    pub light_size_bytes: u64,
    pub light_required_bytes: u64,
    pub high_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LightDiarizationDownloadProgress {
    received: u64,
    total: u64,
    status: String,
    error: Option<String>,
}

#[tauri::command]
pub fn meeting_whisper_model_catalog() -> Vec<CatalogModel> {
    let dir = meeting_whisper_models_dir();
    WHISPER_MODEL_CATALOG
        .iter()
        .map(|(name, _, size)| CatalogModel {
            name: (*name).to_string(),
            size_bytes: *size,
            installed: dir.join(name).is_file(),
        })
        .collect()
}

#[tauri::command]
pub fn meeting_diarization_settings_status() -> MeetingDiarizationSettingsStatus {
    let (segmentation, embedding) = light_diarization_model_paths();
    let segmentation_size = fs::metadata(&segmentation).map(|m| m.len()).unwrap_or(0);
    let embedding_size = fs::metadata(&embedding).map(|m| m.len()).unwrap_or(0);
    MeetingDiarizationSettingsStatus {
        mode: meeting_final_diarization_mode(),
        light_installed: light_diarization_models_installed(),
        light_size_bytes: segmentation_size + embedding_size,
        light_required_bytes: LIGHT_DIARIZATION_SEGMENTATION_BYTES
            + LIGHT_DIARIZATION_EMBEDDING_BYTES,
        high_configured: meeting_final_diarization_command_config()
            .ok()
            .flatten()
            .is_some(),
    }
}

#[tauri::command]
pub async fn meeting_download_light_diarization_model(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || download_light_diarization_model_blocking(&app))
        .await
        .map_err(|e| format!("light speaker download task failed: {e}"))?
}

fn light_diarization_model_dir() -> PathBuf {
    said_core::paths::data_dir()
        .join("models")
        .join("diarization")
}

fn light_diarization_model_paths() -> (PathBuf, PathBuf) {
    let dir = light_diarization_model_dir();
    (
        dir.join(LIGHT_DIARIZATION_SEGMENTATION_NAME),
        dir.join(LIGHT_DIARIZATION_EMBEDDING_NAME),
    )
}

fn light_diarization_models_installed() -> bool {
    let (segmentation, embedding) = light_diarization_model_paths();
    fs::metadata(segmentation)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(false)
        && fs::metadata(embedding)
            .map(|m| m.len() > 10_000_000)
            .unwrap_or(false)
}

fn validate_light_diarization_models() -> Result<(), String> {
    if light_diarization_models_installed() {
        Ok(())
    } else {
        Err("Light speaker detection is not downloaded yet. Open Settings -> Meeting and download the light speaker model.".to_string())
    }
}

fn download_light_diarization_model_blocking(app: &AppHandle) -> Result<(), String> {
    let dir = light_diarization_model_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("couldn't create speaker model folder: {e}"))?;
    let files = [
        (
            LIGHT_DIARIZATION_SEGMENTATION_NAME,
            LIGHT_DIARIZATION_SEGMENTATION_URL,
            LIGHT_DIARIZATION_SEGMENTATION_BYTES,
        ),
        (
            LIGHT_DIARIZATION_EMBEDDING_NAME,
            LIGHT_DIARIZATION_EMBEDDING_URL,
            LIGHT_DIARIZATION_EMBEDDING_BYTES,
        ),
    ];
    let total = files.iter().map(|(_, _, size)| *size).sum::<u64>();
    let mut received_total = 0_u64;
    emit_light_diarization_download(app, received_total, total, "downloading", None);
    for (name, url, size_hint) in files {
        let dest = dir.join(name);
        if dest.is_file() {
            received_total += fs::metadata(&dest).map(|m| m.len()).unwrap_or(size_hint);
            emit_light_diarization_download(
                app,
                received_total.min(total),
                total,
                "downloading",
                None,
            );
            continue;
        }
        download_light_diarization_file(
            app,
            url,
            size_hint,
            &dir,
            &dest,
            &mut received_total,
            total,
        )?;
    }
    emit_light_diarization_download(app, total, total, "done", None);
    Ok(())
}

fn download_light_diarization_file(
    app: &AppHandle,
    url: &str,
    total_hint: u64,
    dir: &Path,
    dest: &Path,
    received_total: &mut u64,
    combined_total: u64,
) -> Result<(), String> {
    use std::io::{Read, Write};

    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("speaker-model");
    let part = dir.join(format!("{name}.part"));
    let fail = |part: &Path, received: u64, total: u64, msg: String| -> String {
        let _ = fs::remove_file(part);
        emit_light_diarization_download(app, received, total, "error", Some(msg.clone()));
        msg
    };
    let client = reqwest::blocking::Client::builder().build().map_err(|e| {
        fail(
            &part,
            *received_total,
            combined_total,
            format!("http client: {e}"),
        )
    })?;
    let mut response = client.get(url).send().map_err(|e| {
        fail(
            &part,
            *received_total,
            combined_total,
            format!("request failed: {e}"),
        )
    })?;
    if !response.status().is_success() {
        return Err(fail(
            &part,
            *received_total,
            combined_total,
            format!("download failed: HTTP {}", response.status()),
        ));
    }
    let file_total = response.content_length().unwrap_or(total_hint);
    let mut file = fs::File::create(&part).map_err(|e| {
        fail(
            &part,
            *received_total,
            combined_total,
            format!("create temp: {e}"),
        )
    })?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut file_received: u64 = 0;
    let mut last_emit = *received_total;
    loop {
        let n = response.read(&mut buf).map_err(|e| {
            fail(
                &part,
                *received_total,
                combined_total,
                format!("read failed: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| {
            fail(
                &part,
                *received_total,
                combined_total,
                format!("write failed: {e}"),
            )
        })?;
        file_received += n as u64;
        let combined_received = *received_total + file_received;
        if combined_received.saturating_sub(last_emit) >= 1_000_000 {
            last_emit = combined_received;
            emit_light_diarization_download(
                app,
                combined_received.min(combined_total),
                combined_total,
                "downloading",
                None,
            );
        }
    }
    file.flush().ok();
    let _ = file.sync_all();
    drop(file);
    if file_total > 0 && file_received < file_total / 2 {
        return Err(fail(
            &part,
            *received_total + file_received,
            combined_total,
            "download ended early (incomplete speaker model)".to_string(),
        ));
    }
    fs::rename(&part, dest).map_err(|e| {
        fail(
            &part,
            *received_total + file_received,
            combined_total,
            format!("finalize: {e}"),
        )
    })?;
    *received_total += file_received;
    Ok(())
}

fn emit_light_diarization_download(
    app: &AppHandle,
    received: u64,
    total: u64,
    status: &str,
    error: Option<String>,
) {
    emit_main(
        app,
        LIGHT_DIARIZATION_EVENT,
        LightDiarizationDownloadProgress {
            received,
            total,
            status: status.to_string(),
            error,
        },
    );
}

#[tauri::command]
pub fn meeting_cancel_model_download(name: String) {
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.insert(name);
    }
}

/// Download a catalogued whisper model with streamed progress events, partial-
/// file cleanup, cancellation, idempotency, and a single-flight guard.
#[tauri::command]
pub async fn meeting_download_whisper_model(app: AppHandle, name: String) -> Result<(), String> {
    let Some((_, url, total_hint)) = WHISPER_MODEL_CATALOG.iter().find(|(n, _, _)| *n == name)
    else {
        return Err(format!("unknown model: {name}"));
    };
    let url = url.to_string();
    let total_hint = *total_hint;
    let dir = meeting_whisper_models_dir();
    let dest = dir.join(&name);
    if dest.is_file() {
        return Ok(()); // already installed — idempotent
    }
    // Single-flight: refuse a concurrent download of the same model.
    {
        let mut inflight = model_downloads_inflight()
            .lock()
            .map_err(|_| "download registry poisoned".to_string())?;
        if !inflight.insert(name.clone()) {
            return Err("this model is already downloading".to_string());
        }
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
    }

    let name_for_task = name.clone();
    let app_dl = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_whisper_model_blocking(&app_dl, &name_for_task, &url, total_hint, &dir, &dest)
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?;

    if let Ok(mut inflight) = model_downloads_inflight().lock() {
        inflight.remove(&name);
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
    }
    if result.is_ok() && !silero_vad_model_installed() {
        let app_auto = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = meeting_download_silero_vad_model(app_auto).await {
                tracing::warn!(
                    "[meeting_engine] auto Silero VAD download after whisper model: {e}"
                );
            }
        });
    }
    result
}

/// Public GGML (fp16) conversion of Oriserve Whisper-Hindi2Hinglish-Swift used by
/// the native, Python-free dictation path. Downloaded into `whisper_model_path()`,
/// which the backend loads at startup.
pub const DICTATION_MODEL_URL: &str = "https://huggingface.co/anish2305/airnote-hinglish-stt-ggml/resolve/main/ggml-oriserve-hinglish-fp16.bin";
const DICTATION_MODEL_SIZE_HINT: u64 = 148_000_000;

#[derive(serde::Serialize)]
pub struct DictationModelStatus {
    pub installed: bool,
    pub size_bytes: u64,
    pub path: String,
}

#[tauri::command]
pub fn dictation_model_status() -> DictationModelStatus {
    let path = said_core::paths::whisper_model_path();
    let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    DictationModelStatus {
        installed: path.is_file(),
        size_bytes,
        path: path.to_string_lossy().to_string(),
    }
}

/// Remove the on-device dictation model file (frees ~148 MB). Idempotent.
#[tauri::command]
pub fn delete_dictation_model() -> Result<(), String> {
    let path = said_core::paths::whisper_model_path();
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| format!("couldn't delete model: {e}"))?;
    }
    Ok(())
}

/// Download the fp16 dictation model into `whisper_model_path()`. Streams with
/// progress on the shared `meeting-model-download` event. Idempotent. Auto-fetches
/// the Silero VAD model afterwards if missing (the native path gates on it).
#[tauri::command]
pub async fn download_dictation_model(app: AppHandle) -> Result<(), String> {
    let dest = said_core::paths::whisper_model_path();
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ggml-oriserve-hinglish-fp16.bin")
        .to_string();
    let dir = dest
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| said_core::paths::data_dir().join("models"));
    if dest.is_file() {
        return Ok(()); // already installed — idempotent
    }
    {
        let mut inflight = model_downloads_inflight()
            .lock()
            .map_err(|_| "download registry poisoned".to_string())?;
        if !inflight.insert(name.clone()) {
            return Err("this model is already downloading".to_string());
        }
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
    }

    let app_dl = app.clone();
    let name_task = name.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_whisper_model_blocking(
            &app_dl,
            &name_task,
            DICTATION_MODEL_URL,
            DICTATION_MODEL_SIZE_HINT,
            &dir,
            &dest,
        )
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?;

    if let Ok(mut inflight) = model_downloads_inflight().lock() {
        inflight.remove(&name);
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
    }
    if result.is_ok() && !silero_vad_model_installed() {
        let app_auto = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = meeting_download_silero_vad_model(app_auto).await {
                tracing::warn!("[meeting_engine] auto Silero VAD after dictation model: {e}");
            }
        });
    }
    result
}

fn download_whisper_model_blocking(
    app: &AppHandle,
    name: &str,
    url: &str,
    total_hint: u64,
    dir: &Path,
    dest: &Path,
) -> Result<(), String> {
    use std::io::{Read, Write};

    fs::create_dir_all(dir).map_err(|e| format!("couldn't create models folder: {e}"))?;
    let part = dir.join(format!("{name}.part"));

    let emit = |received: u64, total: u64, status: &str, error: Option<String>| {
        emit_main(
            app,
            MODEL_DOWNLOAD_EVENT,
            ModelDownloadProgress {
                name: name.to_string(),
                received,
                total,
                status: status.to_string(),
                error,
            },
        );
    };
    let fail = |part: &Path, received: u64, total: u64, msg: String| -> String {
        let _ = fs::remove_file(part);
        emit(received, total, "error", Some(msg.clone()));
        msg
    };

    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| fail(&part, 0, total_hint, format!("http client: {e}")))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| fail(&part, 0, total_hint, format!("request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(fail(
            &part,
            0,
            total_hint,
            format!("download failed: HTTP {}", response.status()),
        ));
    }
    let total = response.content_length().unwrap_or(total_hint);
    let mut file =
        fs::File::create(&part).map_err(|e| fail(&part, 0, total, format!("create temp: {e}")))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut received: u64 = 0;
    let mut last_emit: u64 = 0;
    emit(0, total, "downloading", None);
    loop {
        let cancelled = model_download_cancels()
            .lock()
            .map(|c| c.contains(name))
            .unwrap_or(false);
        if cancelled {
            drop(file);
            let _ = fs::remove_file(&part);
            emit(received, total, "cancelled", None);
            return Err("cancelled".to_string());
        }
        let n = response
            .read(&mut buf)
            .map_err(|e| fail(&part, received, total, format!("read failed: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| fail(&part, received, total, format!("write failed: {e}")))?;
        received += n as u64;
        if received - last_emit >= 2_000_000 {
            last_emit = received;
            emit(received, total, "downloading", None);
        }
    }
    file.flush().ok();
    let _ = file.sync_all();
    drop(file);
    // Sanity: a truncated download (server hiccup / clean stop mid-stream) must NOT
    // masquerade as a complete model. When the server gave a Content-Length, require
    // the FULL byte count — the old `received < total/2` check let a 400MB-of-573MB
    // file pass, get renamed, pass is_usable_whisper_model, then fail opaquely inside
    // whisper-cli at meeting time. (Range/resume + SHA256 are a future enhancement.)
    if total > 0 && received < total {
        return Err(fail(
            &part,
            received,
            total,
            format!("download ended early ({received}/{total} bytes) — please retry"),
        ));
    }
    fs::rename(&part, dest).map_err(|e| fail(&part, received, total, format!("finalize: {e}")))?;
    emit(received, total, "done", None);
    Ok(())
}

/// Delete an installed whisper model. Validates the name (no path traversal,
/// must be a ggml model, never the Silero VAD), and clears the active-model
/// setting if it pointed at the deleted file.
#[tauri::command]
pub fn meeting_delete_whisper_model(name: String) -> Result<(), String> {
    if name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.starts_with("ggml-")
        || !name.ends_with(".bin")
        || name.contains("silero")
    {
        return Err("invalid model name".to_string());
    }
    let path = meeting_whisper_models_dir().join(&name);
    if !path.is_file() {
        return Err("model is not installed".to_string());
    }
    fs::remove_file(&path).map_err(|e| format!("couldn't delete model: {e}"))?;
    let deleted_path = path.display().to_string();
    for key in ["AIRNOTE_WHISPER_CPP_MODEL"] {
        if meeting_env(key).as_deref() == Some(deleted_path.as_str()) {
            let _ = meeting_settings_set(key.to_string(), None);
        }
    }
    // Re-point the active selection to a remaining model (or clear it).
    ensure_active_model_sync();
    Ok(())
}

/// Remove legacy / unsupported meeting whisper models from AirNote's model
/// folder. The current Q5 model and any in-progress Q5 download are preserved.
#[tauri::command]
pub fn meeting_cleanup_legacy_whisper_models() -> Result<WhisperModelCleanupResult, String> {
    let result = cleanup_legacy_whisper_models_in_dir(&meeting_whisper_models_dir())?;
    // If a stale unsupported model had been selected, this re-points to Q5 or
    // clears the setting. It never selects a removed file.
    ensure_active_model_sync();
    Ok(result)
}

// ── Dictation whisper.cpp (shared Turbo Q5 model) ───────────────────────────

pub fn dictation_whisper_model_installed() -> bool {
    selected_whisper_model_path().is_some()
}

pub fn dictation_whisper_runtime_ready() -> bool {
    resolve_whisper_cpp_config().is_ok()
}

pub fn silero_vad_model_installed() -> bool {
    resolve_silero_vad_model_path().is_some()
}

#[derive(Debug, Serialize)]
pub struct SileroVadModelStatus {
    pub name: String,
    pub installed: bool,
    pub size_bytes: u64,
    pub path: String,
}

#[tauri::command]
pub fn silero_vad_model_status() -> SileroVadModelStatus {
    let path = resolve_silero_vad_model_path();
    let installed = path.is_some();
    let size_bytes = path
        .as_ref()
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    SileroVadModelStatus {
        name: SILERO_VAD_MODEL_NAME.to_string(),
        installed,
        size_bytes,
        path: path.map(|p| p.display().to_string()).unwrap_or_default(),
    }
}

/// Download the Silero VAD model whisper.cpp uses for `--vad` speech filtering.
#[tauri::command]
pub async fn meeting_download_silero_vad_model(app: AppHandle) -> Result<(), String> {
    let name = SILERO_VAD_MODEL_NAME.to_string();
    let dir = meeting_whisper_models_dir();
    let dest = dir.join(&name);
    if is_usable_silero_vad_model(&dest) {
        return Ok(());
    }
    {
        let mut inflight = model_downloads_inflight()
            .lock()
            .map_err(|_| "download registry poisoned".to_string())?;
        if !inflight.insert(name.clone()) {
            return Err("Silero VAD is already downloading".to_string());
        }
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
    }

    let app_dl = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_whisper_model_blocking(
            &app_dl,
            SILERO_VAD_MODEL_NAME,
            SILERO_VAD_MODEL_URL,
            SILERO_VAD_SIZE_HINT,
            &dir,
            &dest,
        )
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?;

    if let Ok(mut inflight) = model_downloads_inflight().lock() {
        inflight.remove(SILERO_VAD_MODEL_NAME);
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(SILERO_VAD_MODEL_NAME);
    }
    result
}

#[tauri::command]
pub fn meeting_delete_silero_vad_model() -> Result<(), String> {
    let path = meeting_whisper_models_dir().join(SILERO_VAD_MODEL_NAME);
    if !path.is_file() {
        return Err("Silero VAD model is not installed".to_string());
    }
    fs::remove_file(&path).map_err(|e| format!("couldn't delete Silero VAD model: {e}"))?;
    Ok(())
}

fn is_usable_silero_vad_model(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.len() >= MIN_SILERO_VAD_BYTES)
        .unwrap_or(false)
}

/// Resolve Silero VAD model for whisper.cpp (env, data dir, bundle).
pub fn resolve_silero_vad_model_path() -> Option<PathBuf> {
    env_path("AIRNOTE_MEETING_VAD_MODEL")
        .or_else(|| env_path("AIRNOTE_WHISPER_VAD_MODEL"))
        .filter(|path| is_usable_silero_vad_model(path))
        .or_else(|| {
            selected_whisper_model_path()
                .and_then(|model| model.parent().and_then(find_silero_vad_model))
        })
        .or_else(|| find_silero_vad_model(&meeting_whisper_models_dir()))
        .or_else(|| {
            bundled_models_dirs()
                .iter()
                .find_map(|d| find_silero_vad_model(d))
        })
}

fn dictation_whisper_language(pref_language: &str) -> String {
    match pref_language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => "en".to_string(),
        "hi" | "hindi" | "hinglish" | "" => DEFAULT_WHISPER_LANGUAGE.to_string(),
        other => other.to_string(),
    }
}

fn dictation_whisper_timeout(duration_ms: u64) -> Duration {
    let secs = (duration_ms / 1000).saturating_mul(8).max(30).min(300);
    Duration::from_secs(secs)
}

fn dictation_whisper_live_timeout(duration_ms: u64) -> Duration {
    let secs = (duration_ms / 1000).saturating_mul(4).max(15).min(120);
    Duration::from_secs(secs)
}

fn transcribe_dictation_summary(
    summary: &MicCaptureSummary,
    pref_language: &str,
    timeout: Duration,
) -> Result<String, String> {
    if summary.samples_written == 0 {
        return Err("recording audio is empty".to_string());
    }
    let mut config = resolve_whisper_cpp_config()?;
    config.language = dictation_whisper_language(pref_language);
    let paths = transcript_paths_for_wav(&summary.path);
    let done = transcribe_with_whisper_cpp_for(
        summary,
        &paths,
        &config,
        MeetingAudioTrack::Mic,
        timeout,
        None,
    )?;
    Ok(done.transcript)
}

/// Live/batch dictation STT from 16 kHz mono PCM (used by the live whisper bridge).
pub fn transcribe_dictation_pcm_i16(
    samples: &[i16],
    pref_language: &str,
) -> Result<String, String> {
    if samples.is_empty() {
        return Err("recording audio is empty".to_string());
    }
    let work_dir =
        std::env::temp_dir().join(format!("airnote-dictation-live-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("failed to create dictation temp dir: {e}"))?;
    let wav_path = work_dir.join("dictation.wav");
    let cleanup = || {
        let _ = fs::remove_dir_all(&work_dir);
    };
    let summary = match write_pcm_window_wav(&wav_path, samples.to_vec()) {
        Ok(summary) => summary,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    if !has_transcribable_audio(&summary) {
        cleanup();
        return Err("recording audio is empty".to_string());
    }
    let timeout = dictation_whisper_live_timeout(summary.duration_ms);
    let result = transcribe_dictation_summary(&summary, pref_language, timeout);
    cleanup();
    result
}

/// Offline dictation STT via whisper.cpp using the shared Turbo Q5 model.
pub fn transcribe_dictation_wav_bytes(
    wav_bytes: &[u8],
    pref_language: &str,
) -> Result<String, String> {
    if wav_bytes.len() <= 44 {
        return Err("recording audio is empty".to_string());
    }
    let work_dir = std::env::temp_dir().join(format!("airnote-dictation-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("failed to create dictation temp dir: {e}"))?;
    let wav_path = work_dir.join("dictation.wav");
    let cleanup = || {
        let _ = fs::remove_dir_all(&work_dir);
    };
    fs::write(&wav_path, wav_bytes).map_err(|e| format!("failed to write dictation wav: {e}"))?;
    if let Err(e) = repair_wav_header_sizes(&wav_path) {
        cleanup();
        return Err(format!("failed to repair dictation WAV header: {e}"));
    }
    let summary = match capture_summary_from_wav(&wav_path) {
        Some(summary) => summary,
        None => {
            cleanup();
            return Err("failed to read dictation WAV".to_string());
        }
    };
    let timeout = dictation_whisper_timeout(summary.duration_ms);
    let result = transcribe_dictation_summary(&summary, pref_language, timeout);
    cleanup();
    result
}

fn cleanup_legacy_whisper_models_in_dir(dir: &Path) -> Result<WhisperModelCleanupResult, String> {
    let mut removed = Vec::new();
    let mut freed_bytes = 0_u64;
    if !dir.exists() {
        return Ok(WhisperModelCleanupResult {
            removed,
            freed_bytes,
        });
    }

    for entry in fs::read_dir(dir).map_err(|e| format!("couldn't read models folder: {e}"))? {
        let entry = entry.map_err(|e| format!("couldn't read model entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("couldn't inspect model entry: {e}"))?;
        if !(file_type.is_file() || file_type.is_symlink()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_legacy_meeting_whisper_model_file(&name) {
            continue;
        }
        let path = entry.path();
        let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        fs::remove_file(&path).map_err(|e| format!("couldn't delete {name}: {e}"))?;
        freed_bytes = freed_bytes.saturating_add(size_bytes);
        removed.push(RemovedWhisperModelInfo { name, size_bytes });
    }

    removed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(WhisperModelCleanupResult {
        removed,
        freed_bytes,
    })
}

fn is_legacy_meeting_whisper_model_file(name: &str) -> bool {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    let model_name = name
        .strip_suffix(".part")
        .or_else(|| name.strip_suffix(".tmp"))
        .unwrap_or(name);
    if is_supported_meeting_whisper_model_name(model_name) {
        return false;
    }
    model_name.starts_with("ggml-")
        && model_name.ends_with(".bin")
        && !model_name.contains("silero")
}

/// Guarantee an active model whenever any usable one exists. Keeps the current
/// selection if it's still a real file; otherwise auto-selects the low-memory
/// recommended model and persists it (so a single installed model is always
/// active). Clears the setting if no usable model remains. Returns the active
/// model's file name, or None when none is installed.
// Async (off the main thread): polled every 5s by the Meetings list. See the
// note on get_snapshot — sync commands on Windows block IPC dispatch and starve
// the ~6-connection ipc://localhost pool, which is what wedges End-meeting and
// the "Loading meetings…" spinner after a meeting ends. Delegates to the sync
// core so in-process callers (model delete/cleanup, startup) can still invoke it
// directly without an async context.
#[tauri::command]
pub fn meeting_ensure_active_model() -> Option<String> {
    ensure_active_model_sync()
}

/// Sync core for [`meeting_ensure_active_model`]. Callable from non-async code.
pub fn ensure_active_model_sync() -> Option<String> {
    let name_of = |path: &Path| -> Option<String> {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
    };
    // Keep the current selection if it still points at a real, usable model.
    if let Some(current) = env_path("AIRNOTE_WHISPER_CPP_MODEL") {
        let supported = current
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_supported_meeting_whisper_model_name);
        if supported && is_usable_whisper_model(&current) {
            return name_of(&current);
        }
    }
    // Otherwise auto-select the low-memory recommended model and persist it.
    let dir = meeting_whisper_models_dir();
    if let Some(best) = first_whisper_model_in_dir(&dir) {
        let _ = meeting_settings_set(
            "AIRNOTE_WHISPER_CPP_MODEL".to_string(),
            Some(best.display().to_string()),
        );
        return name_of(&best);
    }
    // No usable model installed — drop any stale selection.
    let _ = meeting_settings_set("AIRNOTE_WHISPER_CPP_MODEL".to_string(), None);
    None
}

fn env_nonempty(name: &str) -> Option<String> {
    meeting_env(name).and_then(|value| {
        let value = value.trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    })
}

fn env_u64(name: &str, default: u64) -> u64 {
    meeting_env(name)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_i32_at_least(name: &str, default: i32, min: i32) -> i32 {
    meeting_env(name)
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value >= min)
        .unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    meeting_env(name)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    meeting_env(name)
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    meeting_env(name)
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn resolve_whisper_cpp_config() -> Result<WhisperCppConfig, String> {
    let binary = env_path("AIRNOTE_WHISPER_CPP_BIN")
        .or_else(|| env_path("WHISPER_CPP_BIN"))
        // Bundled sidecar inside the shipped app. Preferred over PATH so a
        // packaged install transcribes without any dev tooling present.
        .or_else(find_bundled_whisper_cli)
        .or_else(|| find_on_path("whisper-cli"))
        .or_else(|| find_on_path("main"))
        .ok_or_else(|| {
            "whisper.cpp binary not found; set AIRNOTE_WHISPER_CPP_BIN or WHISPER_CPP_BIN"
                .to_string()
        })?;

    let model = selected_whisper_model_path().ok_or_else(|| {
        "whisper.cpp model not found; set AIRNOTE_WHISPER_CPP_MODEL or WHISPER_CPP_MODEL"
            .to_string()
    })?;

    let language = DEFAULT_WHISPER_LANGUAGE.to_string();
    let prompt = meeting_env("AIRNOTE_MEETING_WHISPER_PROMPT")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let max_context_tokens = env_i32_at_least(
        "AIRNOTE_MEETING_WHISPER_MAX_CONTEXT_TOKENS",
        DEFAULT_WHISPER_MAX_CONTEXT_TOKENS,
        -1,
    );
    let suppress_non_speech = env_bool(
        "AIRNOTE_MEETING_WHISPER_SUPPRESS_NON_SPEECH",
        DEFAULT_WHISPER_SUPPRESS_NON_SPEECH,
    );
    let no_fallback = env_bool(
        "AIRNOTE_MEETING_WHISPER_NO_FALLBACK",
        DEFAULT_WHISPER_NO_FALLBACK,
    );
    let no_speech_threshold = env_bool("AIRNOTE_MEETING_WHISPER_NO_SPEECH_GATE", true).then(|| {
        env_f32(
            "AIRNOTE_MEETING_WHISPER_NO_SPEECH_THRESHOLD",
            DEFAULT_WHISPER_NO_SPEECH_THRESHOLD,
        )
    });
    let logprob_threshold = env_bool("AIRNOTE_MEETING_WHISPER_LOGPROB_GATE", true).then(|| {
        env_f32(
            "AIRNOTE_MEETING_WHISPER_LOGPROB_THRESHOLD",
            DEFAULT_WHISPER_LOGPROB_THRESHOLD,
        )
    });
    let entropy_threshold = env_bool("AIRNOTE_MEETING_WHISPER_ENTROPY_GATE", true).then(|| {
        env_f32(
            "AIRNOTE_MEETING_WHISPER_ENTROPY_THRESHOLD",
            DEFAULT_WHISPER_ENTROPY_THRESHOLD,
        )
    });
    let min_segment_confidence =
        env_bool("AIRNOTE_MEETING_WHISPER_CONFIDENCE_GATE", false).then(|| {
            env_f64(
                "AIRNOTE_MEETING_WHISPER_MIN_SEGMENT_CONFIDENCE",
                DEFAULT_WHISPER_MIN_SEGMENT_CONFIDENCE,
            )
            .clamp(0.0, 1.0)
        });

    // VAD on by default; model from env or found beside the whisper model.
    let vad_model = if env_bool("AIRNOTE_MEETING_VAD", true) {
        resolve_silero_vad_model_path()
    } else {
        None
    };
    // VAD is on by default but degrades to off when no Silero model is found.
    // Surface that explicitly so a missing model is diagnosable rather than a
    // silent quality regression.
    if env_bool("AIRNOTE_MEETING_VAD", true) && vad_model.is_none() {
        tracing::warn!(
            "[meeting_engine] VAD enabled but no Silero model found — running whisper without silence filtering"
        );
    }
    let vad_threshold = env_f32("AIRNOTE_MEETING_VAD_THRESHOLD", DEFAULT_VAD_THRESHOLD);
    let vad_speech_pad_ms = env_i32_at_least(
        "AIRNOTE_MEETING_VAD_SPEECH_PAD_MS",
        DEFAULT_VAD_SPEECH_PAD_MS,
        0,
    );
    let vad_min_silence_ms = env_i32_at_least(
        "AIRNOTE_MEETING_VAD_MIN_SILENCE_MS",
        DEFAULT_VAD_MIN_SILENCE_MS,
        0,
    );
    let romanize = env_bool("AIRNOTE_MEETING_ROMANIZE", true);

    tracing::info!(
        vad = vad_model.is_some(),
        vad_model = ?vad_model,
        entropy_threshold = ?entropy_threshold,
        no_fallback,
        romanize,
        "[meeting_engine] whisper config resolved"
    );

    Ok(WhisperCppConfig {
        binary,
        model,
        language,
        max_context_tokens,
        prompt,
        suppress_non_speech,
        no_fallback,
        no_speech_threshold,
        logprob_threshold,
        entropy_threshold,
        min_segment_confidence,
        vad_model,
        vad_threshold,
        vad_speech_pad_ms,
        vad_min_silence_ms,
        romanize,
    })
}

/// Locate a `whisper-cli` binary bundled inside the shipped app. Tauri
/// externalBin strips the target suffix in normal bundles, but dev and failed
/// packaging runs may leave target-suffixed files, so both forms are checked.
/// None if absent (callers then fall back to PATH).
fn find_bundled_whisper_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_whisper_cli_candidates_from_exe(&exe)
        .into_iter()
        .find(|p| p.is_file())
}

fn bundled_whisper_cli_candidates_from_exe(exe: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        if let Some(d) = dir {
            for name in whisper_cli_binary_names() {
                candidates.push(d.join(name));
                candidates.push(d.join("debug").join(name));
                candidates.push(d.join("release").join(name));
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    candidates
}

fn whisper_cli_binary_names() -> [&'static str; 5] {
    [
        "whisper-cli",
        "whisper-cli.exe",
        "whisper-cli-x86_64-pc-windows-msvc.exe",
        "whisper-cli-aarch64-apple-darwin",
        "whisper-cli-x86_64-apple-darwin",
    ]
}

/// Directories inside a shipped `.app` where bundled models may live, relative
/// to the app executable (`Contents/MacOS/<exe>`): the executable's own folder
/// and `Contents/Resources/models`. Empty/absent dirs are harmless — callers
/// scan each for the file they need.
fn bundled_models_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
            dirs.push(d.join("models"));
            dirs.push(d.join("resources").join("models"));
            dirs.push(d.join("..").join("Resources").join("models"));
            dirs.push(d.join("..").join("resources").join("models"));
        }
    }
    dirs
}

/// Find a Silero VAD ggml model (`ggml-silero-*.bin`) in `dir`, preferring the
/// highest-versioned filename. Returns None if none present.
fn find_silero_vad_model(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("ggml-silero") && name.ends_with(".bin") {
            let path = entry.path();
            if is_usable_silero_vad_model(&path) && best.as_ref().map(|b| path > *b).unwrap_or(true)
            {
                best = Some(path);
            }
        }
    }
    best
}

fn resolve_live_transcript_config() -> Result<LiveTranscriptConfig, String> {
    let mut whisper = resolve_whisper_cpp_config()
        .map_err(|e| format!("live transcript requires whisper.cpp; {e}"))?;
    whisper.max_context_tokens = env_i32_at_least(
        "AIRNOTE_MEETING_LIVE_WHISPER_MAX_CONTEXT_TOKENS",
        DEFAULT_LIVE_WHISPER_MAX_CONTEXT_TOKENS,
        -1,
    );

    let context_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_CONTEXT_SECS",
        env_u64(
            "AIRNOTE_MEETING_LIVE_TRANSCRIPT_CHUNK_SECS",
            DEFAULT_LIVE_TRANSCRIPT_CONTEXT_SECS,
        ),
    )
    .clamp(10, 120);
    let step_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_STEP_SECS",
        DEFAULT_LIVE_TRANSCRIPT_STEP_SECS,
    )
    .clamp(3, context_secs);
    let min_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_MIN_SECS",
        DEFAULT_LIVE_TRANSCRIPT_MIN_SECS,
    )
    .clamp(1, step_secs);
    let poll_ms = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_POLL_MS",
        DEFAULT_LIVE_TRANSCRIPT_POLL_MS,
    )
    .clamp(250, 10_000);
    let timeout_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_TIMEOUT_SECS",
        DEFAULT_LIVE_TRANSCRIPT_TIMEOUT_SECS,
    )
    .clamp(10, 15 * 60);

    Ok(LiveTranscriptConfig {
        whisper,
        context_samples: context_secs.saturating_mul(SAMPLE_RATE as u64) as usize,
        step_samples: step_secs.saturating_mul(SAMPLE_RATE as u64) as usize,
        min_samples: min_secs.saturating_mul(SAMPLE_RATE as u64) as usize,
        poll_interval: Duration::from_millis(poll_ms),
        timeout: Duration::from_secs(timeout_secs),
    })
}

/// A whisper ggml model below this is empty/partial/broken (the smallest real
/// model, tiny, is ~75 MB). Used to skip 0-byte placeholders and dangling
/// symlinks so they never get auto-selected or shown as usable.
const MIN_WHISPER_MODEL_BYTES: u64 = 1_000_000;

/// True if `path` is a real, non-trivial whisper model. Follows symlinks (so a
/// symlink to a multi-GB model in another dir counts), rejects empty/partial
/// files and broken symlinks.
fn is_usable_whisper_model(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.len() >= MIN_WHISPER_MODEL_BYTES)
        .unwrap_or(false)
}

fn is_supported_usable_whisper_model(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_supported_meeting_whisper_model_name)
        && is_usable_whisper_model(path)
}

fn env_path(key: &str) -> Option<PathBuf> {
    env_file_path(key)
}

fn env_file_path(key: &str) -> Option<PathBuf> {
    let raw = meeting_env(key)?;
    config_path_candidates(raw.trim())
        .into_iter()
        .find(|path| path.is_file())
}

fn env_executable_path(key: &str) -> Option<PathBuf> {
    let raw = env_nonempty(key)?;
    for path in config_path_candidates(&raw) {
        if path.is_file() {
            return Some(path);
        }
    }
    if !raw.contains('/') && !raw.contains('\\') {
        return find_on_path(&raw);
    }
    None
}

fn env_dir(key: &str) -> Option<PathBuf> {
    let raw = meeting_env(key)?;
    config_path_candidates(raw.trim())
        .into_iter()
        .find(|path| path.is_dir())
}

fn config_path_candidates(raw: &str) -> Vec<PathBuf> {
    let path = expand_tilde(raw);
    if path.is_absolute() {
        return vec![path];
    }

    let mut candidates = Vec::new();
    candidates.push(path.clone());
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(&path));
        }
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);
        candidates.push(manifest_dir.join(&path));
        if let Some(desktop_dir) = manifest_dir.parent() {
            candidates.push(desktop_dir.join(&path));
            if let Some(repo_dir) = desktop_dir.parent() {
                candidates.push(repo_dir.join(&path));
            }
        }
    }

    candidates
}

fn default_whisper_model_path() -> Option<PathBuf> {
    env_dir("AIRNOTE_WHISPER_CPP_MODEL_DIR")
        .or_else(|| env_dir("WHISPER_CPP_MODEL_DIR"))
        .and_then(|dir| first_whisper_model_in_dir(&dir))
        .or_else(default_data_dir_whisper_model_path)
        .or_else(default_dev_repo_whisper_model_path)
}

fn selected_whisper_model_path() -> Option<PathBuf> {
    choose_whisper_model_path(
        env_path("AIRNOTE_WHISPER_CPP_MODEL").or_else(|| env_path("WHISPER_CPP_MODEL")),
        default_whisper_model_path(),
    )
}

fn choose_whisper_model_path(
    configured: Option<PathBuf>,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    configured
        .filter(|path| is_supported_usable_whisper_model(path))
        .or(fallback)
}

fn default_data_dir_whisper_model_path() -> Option<PathBuf> {
    let data_dir = said_core::paths::data_dir();
    default_whisper_model_candidates(&data_dir)
        .into_iter()
        .find(|path| is_usable_whisper_model(path))
}

fn default_dev_repo_whisper_model_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(option_env!("CARGO_MANIFEST_DIR")?);
    let repo_dir = manifest_dir.parent()?.parent()?;
    first_whisper_model_in_dir(&repo_dir.join("tools").join("stt-bench").join("models"))
}

fn first_whisper_model_in_dir(model_dir: &Path) -> Option<PathBuf> {
    whisper_model_candidate_names()
        .into_iter()
        .map(|name| model_dir.join(name))
        .find(|path| is_usable_whisper_model(path))
}

fn default_whisper_model_candidates(data_dir: &Path) -> Vec<PathBuf> {
    let models = data_dir.join("models");
    whisper_model_candidate_names()
        .into_iter()
        .map(|name| models.join(name))
        .collect()
}

fn whisper_model_candidate_names() -> Vec<&'static str> {
    vec!["ggml-large-v3-turbo-q5_0.bin"]
}

fn is_supported_meeting_whisper_model_name(name: &str) -> bool {
    WHISPER_MODEL_CATALOG
        .iter()
        .any(|(catalog_name, _, _)| *catalog_name == name)
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{binary}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

fn clean_whisper_stdout(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("whisper_")
                && !line.starts_with("system_info:")
                && !line.starts_with("main:")
                && !line.starts_with("[")
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[cfg(target_os = "macos")]
fn system_samples_from_buffer(
    sample: &screencapturekit::prelude::CMSampleBuffer,
) -> Option<Vec<f32>> {
    use screencapturekit::prelude::*;

    let _ = sample.make_data_ready();
    let audio = sample.audio_buffer_list()?;
    let mut out = Vec::new();
    for buffer in audio.iter() {
        out.extend(decode_pcm_f32(buffer.data()));
    }
    if out.is_empty() { None } else { Some(out) }
}

fn decode_pcm_f32(bytes: &[u8]) -> Vec<f32> {
    if bytes.len() % 4 == 0 {
        let f32_samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .filter(|sample| sample.is_finite())
            .map(|sample| sample.clamp(-1.0, 1.0))
            .collect();
        if !f32_samples.is_empty() {
            return f32_samples;
        }
    }
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_ne_bytes([chunk[0], chunk[1]]) as f32 / 32_768.0)
        .collect()
}

fn truncate_error(message: &str) -> String {
    const MAX: usize = 800;
    if message.len() <= MAX {
        return message.to_string();
    }
    let mut end = MAX;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &message[..end])
}

fn enqueue_resampled_pcm(
    mono: Vec<f32>,
    native_rate: u32,
    muted: &AtomicBool,
    audio_tx: &mpsc::SyncSender<Vec<i16>>,
    dropped_chunks: &AtomicU64,
    live_audio_tx: Option<&mpsc::SyncSender<LiveAudioChunk>>,
    source: LiveAudioSource,
) {
    if muted.load(Ordering::SeqCst) || mono.is_empty() {
        return;
    }

    let pcm: Vec<i16> = resample_to_16k(&mono, native_rate)
        .into_iter()
        .map(float_to_i16)
        .collect();
    if pcm.is_empty() {
        return;
    }

    enqueue_pcm(pcm, audio_tx, dropped_chunks, live_audio_tx, source);
}

fn enqueue_pcm(
    pcm: Vec<i16>,
    audio_tx: &mpsc::SyncSender<Vec<i16>>,
    dropped_chunks: &AtomicU64,
    live_audio_tx: Option<&mpsc::SyncSender<LiveAudioChunk>>,
    source: LiveAudioSource,
) {
    if pcm.is_empty() {
        return;
    }

    if let Some(live_audio_tx) = live_audio_tx {
        let chunk = LiveAudioChunk {
            source,
            samples: pcm.clone(),
        };
        let _ = live_audio_tx.try_send(chunk);
    }

    if audio_tx.try_send(pcm).is_err() {
        dropped_chunks.fetch_add(1, Ordering::SeqCst);
    }
}

fn mono_from_f32(data: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return data.to_vec();
    }

    data.chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

fn mono_from_i16(data: &[i16], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return data.iter().map(|sample| *sample as f32 / 32768.0).collect();
    }

    data.chunks_exact(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|sample| *sample as i32).sum();
            (sum as f32 / channels as f32) / 32768.0
        })
        .collect()
}

fn mono_from_u16(data: &[u16], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let normalize = |sample: u16| -> f32 { (sample as f32 - 32768.0) / 32768.0 };
    if channels == 1 {
        return data.iter().map(|sample| normalize(*sample)).collect();
    }

    data.chunks_exact(channels)
        .map(|frame| frame.iter().map(|sample| normalize(*sample)).sum::<f32>() / channels as f32)
        .collect()
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsLoopbackSampleFormat {
    F32,
    I16,
    I24,
    I32,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WasapiMixFormat {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bytes_per_sample: u16,
    sample_format: WindowsLoopbackSampleFormat,
}

#[cfg(any(target_os = "windows", test))]
fn pcm_sample_format_for_bits(bits_per_sample: u16) -> Result<WindowsLoopbackSampleFormat, String> {
    match bits_per_sample {
        16 => Ok(WindowsLoopbackSampleFormat::I16),
        24 => Ok(WindowsLoopbackSampleFormat::I24),
        32 => Ok(WindowsLoopbackSampleFormat::I32),
        _ => Err(format!(
            "unsupported WASAPI PCM bits_per_sample={bits_per_sample}"
        )),
    }
}

#[cfg(any(target_os = "windows", test))]
fn min_bytes_per_windows_loopback_sample(sample_format: WindowsLoopbackSampleFormat) -> usize {
    match sample_format {
        WindowsLoopbackSampleFormat::F32 | WindowsLoopbackSampleFormat::I32 => 4,
        WindowsLoopbackSampleFormat::I24 => 3,
        WindowsLoopbackSampleFormat::I16 => 2,
    }
}

#[cfg(any(target_os = "windows", test))]
fn bytes_per_windows_loopback_sample(
    channels: u16,
    block_align: u16,
    sample_format: WindowsLoopbackSampleFormat,
) -> Option<usize> {
    let channels = channels.max(1) as usize;
    let block_align = block_align as usize;
    if block_align == 0 || block_align % channels != 0 {
        return None;
    }
    let bytes_per_sample = block_align / channels;
    if bytes_per_sample < min_bytes_per_windows_loopback_sample(sample_format)
        || bytes_per_sample > 4
    {
        return None;
    }
    Some(bytes_per_sample)
}

#[cfg(any(target_os = "windows", test))]
fn decode_windows_loopback_frames_to_mono(
    bytes: &[u8],
    frame_count: u32,
    channels: u16,
    block_align: u16,
    bytes_per_sample: u16,
    sample_format: WindowsLoopbackSampleFormat,
) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let block_align = block_align as usize;
    let sample_bytes = bytes_per_sample as usize;
    if block_align == 0
        || sample_bytes == 0
        || sample_bytes < min_bytes_per_windows_loopback_sample(sample_format)
        || sample_bytes > 4
    {
        return Vec::new();
    }
    let frames = (frame_count as usize).min(bytes.len() / block_align);
    let mut mono = Vec::with_capacity(frames);
    for frame_index in 0..frames {
        let frame_start = frame_index * block_align;
        let frame_end = frame_start + block_align;
        let mut sum = 0.0_f32;
        let mut seen = 0_usize;
        for channel_index in 0..channels {
            let offset = frame_start + channel_index * sample_bytes;
            if offset + sample_bytes > frame_end || offset + sample_bytes > bytes.len() {
                break;
            }
            sum += decode_windows_loopback_sample(
                &bytes[offset..offset + sample_bytes],
                sample_format,
            );
            seen += 1;
        }
        if seen > 0 {
            mono.push(sum / seen as f32);
        }
    }
    mono
}

#[cfg(any(target_os = "windows", test))]
fn decode_windows_loopback_sample(bytes: &[u8], sample_format: WindowsLoopbackSampleFormat) -> f32 {
    match sample_format {
        WindowsLoopbackSampleFormat::F32 => {
            if bytes.len() < 4 {
                return 0.0;
            }
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }
        WindowsLoopbackSampleFormat::I16
        | WindowsLoopbackSampleFormat::I24
        | WindowsLoopbackSampleFormat::I32 => decode_signed_pcm_container(bytes),
    }
}

#[cfg(any(target_os = "windows", test))]
fn decode_signed_pcm_container(bytes: &[u8]) -> f32 {
    match bytes.len() {
        2 => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0,
        3 => {
            let raw = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
            let signed = (raw << 8) >> 8;
            signed as f32 / 8_388_608.0
        }
        4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0,
        _ => 0.0,
    }
}

fn float_to_i16(sample: f32) -> i16 {
    let clipped = sample.clamp(-1.0, 1.0);
    if clipped >= 0.0 {
        (clipped * i16::MAX as f32) as i16
    } else {
        (clipped * 32768.0) as i16
    }
}

fn system_capture_status(
    active: bool,
    system_running: bool,
    summary: &Option<SystemCaptureSummary>,
    error: &Option<String>,
) -> String {
    if system_running {
        "recording"
    } else if error.is_some() {
        "unavailable"
    } else if summary.is_some() {
        "stopped"
    } else if active {
        "not_started"
    } else {
        "idle"
    }
    .to_string()
}

fn status_reason(
    active: bool,
    muted: bool,
    mic_running: bool,
    system_running: bool,
    last_error: Option<&str>,
) -> String {
    if last_error.is_some() {
        "mic_capture_error"
    } else if active && muted {
        "mic_capture_muted"
    } else if active && mic_running && system_running {
        "mic_system_capture_recording"
    } else if active && mic_running {
        "mic_capture_recording"
    } else if active && system_running {
        "system_audio_capture_recording"
    } else if active {
        "meeting_engine_ready"
    } else {
        "meeting_engine_stopped"
    }
    .to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_whisper_candidates_include_windows_exe_names() {
        let candidates = bundled_whisper_cli_candidates_from_exe(Path::new(
            "/tmp/AirNote.app/Contents/MacOS/AirNote",
        ));

        assert!(candidates.contains(&PathBuf::from(
            "/tmp/AirNote.app/Contents/MacOS/whisper-cli"
        )));
        assert!(candidates.contains(&PathBuf::from(
            "/tmp/AirNote.app/Contents/MacOS/whisper-cli.exe"
        )));
        assert!(candidates.contains(&PathBuf::from(
            "/tmp/AirNote.app/Contents/MacOS/whisper-cli-x86_64-pc-windows-msvc.exe"
        )));
        assert!(candidates.contains(&PathBuf::from(
            "/tmp/AirNote.app/Contents/MacOS/release/whisper-cli.exe"
        )));
    }

    #[test]
    fn windows_taskkill_args_adds_tree_and_force_flags() {
        assert_eq!(
            windows_taskkill_args(9182, false),
            vec!["/PID", "9182", "/T"]
        );
        assert_eq!(
            windows_taskkill_args(9182, true),
            vec!["/PID", "9182", "/T", "/F"]
        );
    }

    #[test]
    fn windows_loopback_decoder_downmixes_f32_stereo() {
        let mut bytes = Vec::new();
        for sample in [0.25_f32, 0.75, -0.5, 0.0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let mono = decode_windows_loopback_frames_to_mono(
            &bytes,
            2,
            2,
            8,
            4,
            WindowsLoopbackSampleFormat::F32,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.25).abs() < 0.0001);
    }

    #[test]
    fn windows_loopback_decoder_downmixes_i16_stereo() {
        let mut bytes = Vec::new();
        for sample in [16_384_i16, 16_384, -32_768, 0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let mono = decode_windows_loopback_frames_to_mono(
            &bytes,
            2,
            2,
            4,
            2,
            WindowsLoopbackSampleFormat::I16,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.5).abs() < 0.0001);
    }

    #[test]
    fn windows_loopback_decoder_handles_signed_i24() {
        let bytes = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let mono = decode_windows_loopback_frames_to_mono(
            &bytes,
            2,
            1,
            3,
            3,
            WindowsLoopbackSampleFormat::I24,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.5).abs() < 0.0001);
    }

    #[test]
    fn windows_loopback_decoder_handles_left_aligned_i24_in_i32_container() {
        let bytes = [
            0x00, 0x00, 0x00, 0x40, // +0.5 as 24 valid bits left-aligned in i32
            0x00, 0x00, 0x00, 0xC0, // -0.5 as 24 valid bits left-aligned in i32
        ];
        let mono = decode_windows_loopback_frames_to_mono(
            &bytes,
            2,
            1,
            4,
            4,
            WindowsLoopbackSampleFormat::I24,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.5).abs() < 0.0001);
    }

    #[test]
    fn windows_loopback_container_width_comes_from_block_align() {
        assert_eq!(
            bytes_per_windows_loopback_sample(2, 8, WindowsLoopbackSampleFormat::I24),
            Some(4)
        );
        assert_eq!(
            bytes_per_windows_loopback_sample(2, 6, WindowsLoopbackSampleFormat::I24),
            Some(3)
        );
        assert_eq!(
            bytes_per_windows_loopback_sample(2, 4, WindowsLoopbackSampleFormat::I24),
            None
        );
    }

    #[test]
    fn classify_meeting_job_error_routes_transient_vs_terminal() {
        let retry = |m: &str| matches!(classify_meeting_job_error(m), JobOutcome::Retry(_));
        let terminal = |m: &str| matches!(classify_meeting_job_error(m), JobOutcome::Terminal(_));

        // Transient — must retry, never permanently fail the meeting.
        assert!(retry("meeting AI rate-limited (429) by 'deepseek'."));
        assert!(retry(
            "meeting AI request to 'deepseek' failed: operation timed out"
        ));
        assert!(retry("network connection reset"));
        assert!(retry("disk write failed: no space left on device"));

        // Terminal — looping won't help; fail fast with the clear message.
        assert!(terminal("whisper.cpp timed out after 900s"));
        assert!(terminal(
            "meeting AI authentication failed (401) for 'deepseek'"
        ));
        assert!(terminal(
            "whisper model file is missing or corrupt: /m.bin — reinstall it from Settings → Meeting"
        ));
        assert!(terminal("whisper.cpp crashed (exit signal 11, no output)"));
        assert!(terminal(
            "whisper.cpp returned no confident speech transcript"
        ));
        assert!(terminal(
            "whisper.cpp binary not found at /x — engine missing"
        ));

        // A rate-limit that mentions a terminal-ish word still retries.
        assert!(retry("rate limit reached; response was empty this minute"));
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_kills_child_process_group() {
        let marker = std::env::temp_dir().join(format!(
            "airnote-timeout-child-{}-{}.pid",
            std::process::id(),
            now_ms()
        ));
        let script = format!("sleep 30 & echo $! > '{}'; wait", marker.display());
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.process_group(0);
        let child = cmd.spawn().expect("spawn shell child");

        let sleep_pid = wait_for_pid_file(&marker).expect("sleep pid marker");
        let err = wait_with_timeout_for(child, Duration::from_millis(50), "test child")
            .expect_err("child should time out");
        assert!(err.contains("test child timed out"));
        assert_eventually_not_alive(sleep_pid);

        let _ = fs::remove_file(marker);
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_cancel_kills_child_process_group() {
        let marker = std::env::temp_dir().join(format!(
            "airnote-cancel-child-{}-{}.pid",
            std::process::id(),
            now_ms()
        ));
        let script = format!("sleep 30 & echo $! > '{}'; wait", marker.display());
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.process_group(0);
        let child = cmd.spawn().expect("spawn shell child");

        let sleep_pid = wait_for_pid_file(&marker).expect("sleep pid marker");
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let err = wait_with_timeout_for_cancel(
            child,
            Duration::from_secs(30),
            "test child",
            Some(&|| cancel.load(Ordering::SeqCst)),
        )
        .expect_err("child should be cancelled");
        assert!(err.contains("test child cancelled"));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancellation should not wait for child timeout"
        );
        assert_eventually_not_alive(sleep_pid);

        let _ = fs::remove_file(marker);
    }

    #[cfg(unix)]
    fn wait_for_pid_file(path: &Path) -> Option<libc::pid_t> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if let Ok(contents) = fs::read_to_string(path) {
                if let Ok(pid) = contents.trim().parse::<libc::pid_t>() {
                    return Some(pid);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[cfg(unix)]
    fn assert_eventually_not_alive(pid: libc::pid_t) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            if !alive {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("process {pid} was still alive after timeout cleanup");
    }

    #[test]
    fn meeting_llm_retry_delay_honors_retry_after_and_caps() {
        // 429 with a sane Retry-After is honored.
        assert_eq!(
            meeting_llm_retry_delay(429, Some("10"), 1),
            Duration::from_secs(10)
        );
        // 429 with an excessive Retry-After is clamped to the 30s ceiling.
        assert_eq!(
            meeting_llm_retry_delay(429, Some("600"), 1),
            Duration::from_secs(30)
        );
        // 429 with no header → exponential (2s, then 4s).
        assert_eq!(
            meeting_llm_retry_delay(429, None, 1),
            Duration::from_millis(2_000)
        );
        assert_eq!(
            meeting_llm_retry_delay(429, None, 2),
            Duration::from_millis(4_000)
        );
        // Non-429 transient (5xx / network) uses the shorter ramp.
        assert_eq!(
            meeting_llm_retry_delay(500, None, 1),
            Duration::from_millis(800)
        );
    }

    #[test]
    fn start_creates_session_without_audio_tracks_when_capture_disabled() {
        let state = MeetingEngineState::new();

        let status = state.start_session(false);

        assert!(status.active);
        assert!(!status.muted);
        assert!(!status.capture_running);
        assert!(!status.mic_track_active);
        assert!(!status.system_track_active);
        assert_eq!(status.phase, PHASE);
        assert_eq!(status.last_gate_reason, "meeting_engine_ready");
        assert!(status.session_id.is_some());
        assert!(status.started_at_ms.is_some());
        assert!(status.mic_wav_path.is_some());
        assert!(status.system_wav_path.is_some());
        assert_eq!(status.system_capture_status, "not_started");
    }

    #[test]
    fn cleanup_volume_band_rejects_rewrites_keeps_corrections() {
        let raw = "so um we we kicked off the the project and reviewed timeline";
        // A light correction (filler/dup removal) stays in band.
        assert!(cleanup_within_volume_band(
            raw,
            "So we kicked off the project and reviewed the timeline."
        ));
        // A collapse (model dropped most content) is rejected.
        assert!(!cleanup_within_volume_band(raw, "Project kickoff."));
        // A balloon (model rewrote/expanded) is rejected.
        assert!(!cleanup_within_volume_band(
            "ok done",
            "Okay, the team confirmed that everything is now fully done and \
             completed across all of the outstanding items and follow ups today"
        ));
    }

    #[test]
    fn evidence_gate_requires_contiguous_phrase_not_scattered_words() {
        let transcript = "[00:01 You] we kicked off the project and reviewed the timeline \
            then the team agreed to ship the beta on friday after the security review";
        // Real contiguous quote → accepted.
        assert!(evidence_quote_matches_transcript(
            "agreed to ship the beta on friday",
            transcript
        ));
        // Fabricated quote assembled from common words scattered through the
        // transcript → rejected (no 5-token contiguous run).
        assert!(!evidence_quote_matches_transcript(
            "we should the team after the review on monday budget",
            transcript
        ));
        // Exact substring still passes.
        assert!(evidence_quote_matches_transcript(
            "the security review",
            transcript
        ));
    }

    #[test]
    fn cleanup_removes_empty_dir_but_keeps_audio() {
        let base = std::env::temp_dir().join(format!(
            "airnote-cleanup-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let empty = base.join("empty");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join(MEETING_STATE_FILE), b"{}").unwrap();
        cleanup_empty_session_dir(&empty);
        assert!(!empty.exists(), "empty placeholder dir should be removed");

        let with_audio = base.join("with_audio");
        fs::create_dir_all(&with_audio).unwrap();
        fs::write(with_audio.join("mic.wav"), b"RIFFxxxx").unwrap();
        cleanup_empty_session_dir(&with_audio);
        assert!(with_audio.exists(), "dir with audio must be kept");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_removes_intermediates_keeps_sources_and_transcript() {
        let dir =
            std::env::temp_dir().join(format!("airnote-prune-{}-{}", std::process::id(), now_ms()));
        fs::create_dir_all(dir.join("live")).unwrap();
        fs::create_dir_all(dir.join("mic.asr-chunks")).unwrap();
        fs::write(dir.join("live").join("live-mic-0.wav"), b"x").unwrap();
        fs::write(dir.join("mic.asr-chunks").join("chunk-00000.wav"), b"x").unwrap();
        fs::write(dir.join("mic.asr.wav"), b"x").unwrap();
        fs::write(dir.join("system.asr.wav"), b"x").unwrap();
        fs::write(dir.join("mic.wav"), b"x").unwrap();
        fs::write(dir.join("meeting.transcript.json"), b"{}").unwrap();
        assert!(dir_has_asr_copy(&dir));
        prune_meeting_intermediates(&dir);
        assert!(!dir.join("live").exists(), "live/ windows pruned");
        assert!(
            !dir.join("mic.asr-chunks").exists(),
            "final ASR chunks pruned"
        );
        assert!(!dir.join("mic.asr.wav").exists(), ".asr.wav pruned");
        assert!(!dir.join("system.asr.wav").exists());
        assert!(dir.join("mic.wav").exists(), "source WAV kept");
        assert!(
            dir.join("meeting.transcript.json").exists(),
            "transcript kept"
        );
        assert!(!dir_has_asr_copy(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_asr_chunk_writer_splits_wav_with_offsets() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-final-asr-chunks-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let wav_path = dir.join("mic.wav");
        let samples = (0..(SAMPLE_RATE as usize * 2))
            .map(|index| {
                if index < SAMPLE_RATE as usize {
                    1_000_i16
                } else {
                    2_000_i16
                }
            })
            .collect::<Vec<_>>();
        write_test_wav(&wav_path, &samples).unwrap();
        let cache_dir = dir.join("mic.asr-chunks");
        fs::create_dir_all(&cache_dir).unwrap();
        let cached_paths = transcript_paths_for_stem(&cache_dir, "chunk-00000");
        fs::write(&cached_paths.text, "cached chunk transcript").unwrap();
        let summary = MicCaptureSummary {
            path: wav_path,
            samples_written: samples.len() as u64,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 2_000,
            peak: 2_000.0 / i16::MAX as f32,
        };

        let chunks = write_wav_asr_chunks(&summary, 1_000).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(final_asr_chunk_count(&summary, 1_000), chunks.len() as u64);
        assert!(
            cached_paths.text.is_file(),
            "completed chunk transcript cache is preserved for resume"
        );
        assert_eq!(chunks[0].start_ms, 0);
        assert_eq!(chunks[0].summary.duration_ms, 1_000);
        assert_eq!(chunks[0].summary.samples_written, SAMPLE_RATE as u64);
        assert_eq!(chunks[1].start_ms, 1_000);
        assert_eq!(chunks[1].summary.duration_ms, 1_000);
        assert!(chunks[0].summary.path.is_file());
        assert!(chunks[1].summary.path.is_file());
        assert!(chunks[0].summary.path.ends_with("chunk-00000.wav"));
        assert!(chunks[1].summary.path.ends_with("chunk-00001.wav"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_is_idempotent_for_an_active_session() {
        let state = MeetingEngineState::new();

        let first = state.start_session(false);
        let second = state.start_session(false);

        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.started_at_ms, second.started_at_ms);
        assert_eq!(second.generation, 1);
    }

    #[test]
    fn fake_mic_capture_marks_session_as_recording() {
        let state = MeetingEngineState::new();
        state.start_session(false);
        state.install_fake_mic_capture_for_test();

        let status = state.status();

        assert!(status.active);
        assert!(status.capture_running);
        assert!(status.mic_track_active);
        assert_eq!(status.last_gate_reason, "mic_capture_recording");
    }

    #[test]
    fn fake_mic_and_system_capture_mark_session_as_two_track_recording() {
        let state = MeetingEngineState::new();
        state.start_session(false);
        state.install_fake_mic_capture_for_test();
        state.install_fake_system_capture_for_test();

        let status = state.status();

        assert!(status.active);
        assert!(status.capture_running);
        assert!(status.mic_track_active);
        assert!(status.system_track_active);
        assert_eq!(status.system_capture_status, "recording");
        assert_eq!(status.last_gate_reason, "mic_system_capture_recording");
    }

    #[test]
    fn fake_system_capture_without_mic_marks_system_audio_recording() {
        let state = MeetingEngineState::new();
        state.start_session(false);
        state.install_fake_system_capture_for_test();

        let status = state.status();

        assert!(status.active);
        assert!(status.capture_running);
        assert!(!status.mic_track_active);
        assert!(status.system_track_active);
        assert_eq!(status.last_gate_reason, "system_audio_capture_recording");
    }

    #[test]
    fn mute_and_resume_update_status() {
        let state = MeetingEngineState::new();
        state.start_session(false);
        state.install_fake_mic_capture_for_test();

        let muted = state.toggle_mute();
        assert!(muted.active);
        assert!(muted.muted);
        assert!(!muted.capture_running);
        assert!(muted.mic_track_active);
        assert_eq!(muted.last_gate_reason, "mic_capture_muted");

        let resumed = state.toggle_mute();
        assert!(resumed.active);
        assert!(!resumed.muted);
        assert!(resumed.capture_running);
        assert_eq!(resumed.last_gate_reason, "mic_capture_recording");
    }

    #[test]
    fn stop_clears_session() {
        let state = MeetingEngineState::new();
        state.start_session(false);

        let stopped = state.stop();

        assert!(!stopped.active);
        assert!(!stopped.muted);
        assert_eq!(stopped.last_gate_reason, "meeting_engine_stopped");
        assert!(stopped.session_id.is_none());
        assert!(stopped.started_at_ms.is_none());
    }

    fn dummy_meeting_plan(dir: &Path) -> MeetingTranscriptionPlan {
        let wav = dir.join("mic.wav");
        let summary = MicCaptureSummary {
            path: wav.clone(),
            samples_written: 1,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 1,
            peak: 0.1,
        };
        MeetingTranscriptionPlan {
            mic: summary.clone(),
            system: None,
            summary: summary.clone(),
            output_paths: transcript_paths_for_stem(dir, "meeting"),
            source_wavs: vec![wav],
            source_activity_path: None,
        }
    }

    #[test]
    fn meeting_job_queue_cancel_removes_pending_and_marks_in_flight() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-job-cancel-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let jobs = MeetingJobQueue::new();
        assert_eq!(
            jobs.enqueue(MeetingJob {
                meeting_id: "queued-meeting".to_string(),
                plan: Box::new(dummy_meeting_plan(&dir)),
                attempt: 0,
                not_before_ms: 0,
            }),
            EnqueueOutcome::Enqueued
        );

        assert!(jobs.cancel("queued-meeting"));
        assert!(!jobs.is_active("queued-meeting"));

        jobs.lock().in_flight = Some("running-meeting".to_string());
        assert!(jobs.cancel("running-meeting"));
        assert!(jobs.is_cancelled("running-meeting"));
        assert!(jobs.is_active("running-meeting"));
        jobs.clear_cancelled("running-meeting");
        assert!(!jobs.is_cancelled("running-meeting"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn meeting_job_queue_shutdown_drains_pending_without_user_cancelling_in_flight() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-job-shutdown-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let jobs = MeetingJobQueue::new();
        assert_eq!(
            jobs.enqueue(MeetingJob {
                meeting_id: "queued-meeting".to_string(),
                plan: Box::new(dummy_meeting_plan(&dir)),
                attempt: 0,
                not_before_ms: 0,
            }),
            EnqueueOutcome::Enqueued
        );
        jobs.lock().in_flight = Some("running-meeting".to_string());

        let interrupted = jobs.drain_for_shutdown();

        assert!(jobs.is_shutting_down());
        assert_eq!(interrupted, vec!["queued-meeting", "running-meeting"]);
        assert!(!jobs.is_active("queued-meeting"));
        assert!(jobs.is_active("running-meeting"));
        assert!(
            !jobs.is_cancelled("running-meeting"),
            "shutdown is resumable interruption, not a user cancel"
        );

        jobs.lock().in_flight = None;
        jobs.cvar.notify_all();
        assert!(jobs.wait_until_idle(Duration::from_millis(50)));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn shutdown_interruption_marker_stays_resumable_and_preserves_terminal_state() {
        let meeting_id = format!("local-{}-shutdown-marker", now_ms());
        let dir = meeting_dir_for_id(&meeting_id).unwrap();
        fs::create_dir_all(&dir).unwrap();
        write_meeting_state(&dir, MEETING_PHASE_TRANSCRIBING, None);

        mark_meeting_interrupted_for_recovery(&meeting_id, "test interruption");

        let state = read_meeting_state(&dir).unwrap();
        assert_eq!(state.phase, MEETING_PHASE_TRANSCRIBING);
        assert_eq!(state.error.as_deref(), Some("test interruption"));

        write_meeting_state(&dir, MEETING_PHASE_SUMMARIZED, None);
        mark_meeting_interrupted_for_recovery(&meeting_id, "should not overwrite");

        let state = read_meeting_state(&dir).unwrap();
        assert_eq!(state.phase, MEETING_PHASE_SUMMARIZED);
        assert_eq!(state.error, None);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn active_processing_status_does_not_show_done_from_stale_artifacts() {
        let state = MeetingEngineState::new();
        let meeting_id = format!("local-{}-status-test", now_ms());
        let dir = meeting_dir_for_id(&meeting_id).unwrap();
        fs::create_dir_all(&dir).unwrap();
        write_meeting_state(&dir, MEETING_PHASE_SUMMARIZED, None);
        write_test_wav(&dir.join("mic.wav"), &[1_000, 0]).unwrap();
        fs::write(dir.join("meeting.ai.json"), "{}").unwrap();
        {
            let mut inner = state.jobs.lock();
            inner.in_flight = Some(meeting_id.clone());
        }
        {
            let mut snapshot = state.transcription.lock_recover();
            snapshot.status = "running".to_string();
        }

        let status = meeting_processing_status(&state, meeting_id.clone()).unwrap();

        assert!(status.running);
        assert_eq!(status.stage, "transcribing");
        assert_ne!(status.stage, MEETING_PHASE_SUMMARIZED);
        assert!(status.has_intelligence);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn converts_native_samples_to_mono_f32() {
        assert_eq!(mono_from_f32(&[1.0, -1.0, 0.5, 0.0], 2), vec![0.0, 0.25]);
        assert_eq!(mono_from_i16(&[32767, -32768], 1).len(), 2);
        assert_eq!(mono_from_u16(&[65535, 0], 1).len(), 2);
    }

    #[test]
    fn decodes_screencapturekit_float_pcm_bytes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.5_f32.to_ne_bytes());
        bytes.extend_from_slice(&(-2.0_f32).to_ne_bytes());
        bytes.extend_from_slice(&f32::NAN.to_ne_bytes());

        let decoded = decode_pcm_f32(&bytes);

        assert_eq!(decoded, vec![0.5, -1.0]);
    }

    #[test]
    fn meeting_merged_wav_uses_merged_transcript_paths() {
        let paths = transcript_paths_for_wav(Path::new("/tmp/meeting.merged.wav"));

        assert!(paths.text.ends_with("meeting.merged.transcript.txt"));
        assert!(paths.json.ends_with("meeting.merged.transcript.json"));
        assert!(paths.whisper_out_base.ends_with("meeting.merged.whisper"));
    }

    #[test]
    fn live_track_buffer_uses_rolling_context() {
        let second = SAMPLE_RATE as usize;
        let mut buffer = LiveTrackBuffer::new(LiveAudioSource::Mic);
        let context = 45 * second;
        let step = 12 * second;
        let min = 2 * second;

        buffer.push(vec![1; step]);
        let first = buffer
            .take_ready_window(context, step, min, false)
            .expect("first step should be ready");
        assert_eq!(first.start_sample, 0);
        assert_eq!(first.emit_from_sample, 0);
        assert_eq!(first.samples.len(), step);

        buffer.push(vec![2; step]);
        let second_window = buffer
            .take_ready_window(context, step, min, false)
            .expect("second step should be ready");
        assert_eq!(second_window.start_sample, 0);
        assert_eq!(second_window.emit_from_sample, step as u64);
        assert_eq!(second_window.samples.len(), 2 * step);

        buffer.push(vec![3; 30 * second]);
        let rolled = buffer
            .take_ready_window(context, step, min, false)
            .expect("third step should include only rolling context");
        assert_eq!(rolled.start_sample, 9 * SAMPLE_RATE as u64);
        assert_eq!(rolled.emit_from_sample, 24 * SAMPLE_RATE as u64);
        assert_eq!(rolled.samples.len(), context);
    }

    #[test]
    fn meeting_whisper_language_is_hindi_for_all_tracks() {
        assert_eq!(
            whisper_language_for_track(MeetingAudioTrack::Mic, DEFAULT_WHISPER_LANGUAGE),
            "hi"
        );
        assert_eq!(
            whisper_language_for_track(MeetingAudioTrack::System, DEFAULT_WHISPER_LANGUAGE),
            "hi"
        );
    }

    #[test]
    fn mixes_i16_samples_with_clipping() {
        assert_eq!(mix_i16_samples(1_000, -500), 500);
        assert_eq!(mix_i16_samples(i16::MAX, 1), i16::MAX);
        assert_eq!(mix_i16_samples(i16::MIN, -1), i16::MIN);
    }

    #[test]
    fn merge_levels_quiet_mic_against_loud_system() {
        // Real case: mic peak 0.18, system peak 0.90.
        let (mic_gain, system_gain) = merge_mix_gains(0.18, 0.90);
        // Mic is boosted (~0.6/0.18 ≈ 3.3x) so it's audible.
        assert!(mic_gain > 3.0 && mic_gain <= MERGE_MIC_MAX_GAIN);
        // Loud system is attenuated toward the target (~0.6/0.90 ≈ 0.67x).
        assert!(system_gain < 1.0 && system_gain > 0.5);
        // Quiet/normal tracks are not cut.
        assert_eq!(merge_mix_gains(0.5, 0.5).1, 1.0);
        // Silent mic gets no boost.
        assert_eq!(merge_mix_gains(0.0, 0.9).0, 1.0);
    }

    #[test]
    fn asr_gain_boosts_quiet_tracks_but_leaves_loud_tracks() {
        // Already at/above the clip ceiling — no gain.
        assert_eq!(asr_gain_for_peak(0.95), 1.0);
        assert_eq!(asr_gain_for_peak(0.0), 1.0);
        // Below the floor — no gain (can't tell signal from noise).
        assert_eq!(asr_gain_for_peak(0.0005), 1.0);
        assert_eq!(asr_gain_for_peak(0.001), 1.0);
        // Very quiet → clamped to the bounded max (no more 64x).
        assert_eq!(asr_gain_for_peak(0.005), ASR_MAX_GAIN);
        assert!(asr_gain_for_peak(0.005) <= ASR_MAX_GAIN);
    }

    #[test]
    fn asr_gain_for_levels_targets_loudness_without_clipping() {
        // Loud, healthy track — barely any gain.
        let g = asr_gain_for_levels(0.90, 0.040);
        assert!((1.0..=1.2).contains(&g), "loud track gain {g}");
        // Genuinely quiet but real voice (decent RMS) — gained toward target,
        // but limited so the peak never clips.
        let g = asr_gain_for_levels(0.08, 0.020);
        assert!(
            g > 1.0 && g <= ASR_TARGET_PEAK / 0.08 + 0.01,
            "quiet voice gain {g}"
        );
        // Near-silent / noise-dominated — gain is hard-capped, never the old 64x.
        let g = asr_gain_for_levels(0.019, 0.0007);
        assert!(g <= ASR_MAX_GAIN, "noise gain {g} must stay capped");
        // Degenerate inputs.
        assert_eq!(asr_gain_for_levels(0.0, 0.0), 1.0);
        assert_eq!(asr_gain_for_levels(f32::NAN, 0.01), 1.0);
    }

    #[test]
    fn rms_silence_gate_drops_noise_but_keeps_speech() {
        // The real Hindi meeting's mic: high-ish transient peak, silence RMS.
        // Must be gated (RMS below floor) so it isn't hallucinated.
        assert!(0.0007 < ASR_MIN_RMS_FOR_TRANSCRIPTION);
        // The system track: real speech energy — must pass.
        assert!(0.042 >= ASR_MIN_RMS_FOR_TRANSCRIPTION);
    }

    #[test]
    fn chat_context_passes_short_transcript_whole() {
        let t = "[00:01 You] hello world\n[00:02 Speaker 1] yes indeed";
        assert_eq!(assemble_chat_transcript_context(t, "hello", 48_000), t);
    }

    #[test]
    fn chat_context_keeps_relevant_and_recent_when_over_budget() {
        let mut lines =
            vec!["[00:01 You] the marketing budget is fifty thousand dollars".to_string()];
        for i in 0..400 {
            lines.push(format!(
                "[01:{i:03} Speaker 1] filler chatter number {i} about nothing in particular today"
            ));
        }
        let transcript = lines.join("\n");
        let budget = 4_000;
        let ctx =
            assemble_chat_transcript_context(&transcript, "what is the marketing budget?", budget);
        assert!(ctx.len() <= budget + 64, "within budget, got {}", ctx.len());
        assert!(
            ctx.contains("marketing budget is fifty thousand"),
            "keeps the relevant early line"
        );
        assert!(
            ctx.contains("number 399"),
            "keeps the recency tail (latest lines)"
        );
        assert!(ctx.contains("[…]"), "marks omitted spans");
        assert!(
            ctx.len() < transcript.len(),
            "actually excerpted a long transcript"
        );
    }

    /// Runs the REAL gate against the REAL recorded WAVs of a problematic
    /// Hindi meeting, so you can watch the silence-guard fire on actual audio.
    /// Ignored by default (depends on local user data). Run with:
    ///   AIRNOTE_GATE_DEMO_DIR="$HOME/Library/Application Support/VoicePolish/meetings/0626fc32-fbdc-4842-a4df-a0eb3caaee30" \
    ///   cargo test -p said-desktop gate_fires_on_real_meeting_audio -- --ignored --nocapture
    #[test]
    #[ignore]
    fn gate_fires_on_real_meeting_audio() {
        let Some(dir) = std::env::var_os("AIRNOTE_GATE_DEMO_DIR").map(PathBuf::from) else {
            eprintln!("set AIRNOTE_GATE_DEMO_DIR to a meeting dir with mic.wav + system.wav");
            return;
        };
        for track in ["mic", "system"] {
            let path = dir.join(format!("{track}.wav"));
            if !path.is_file() {
                eprintln!("skip {track}: {path:?} not found");
                continue;
            }
            let (peak, rms) = analyze_wav_levels(&path).expect("level scan");
            let summary = MicCaptureSummary {
                path: path.clone(),
                samples_written: 1,
                dropped_chunks: 0,
                native_rate: SAMPLE_RATE,
                duration_ms: 0,
                peak,
            };
            let decision = has_transcribable_audio(&summary);
            let gain = if rms > 0.0 {
                asr_gain_for_levels(peak, rms)
            } else {
                1.0
            };
            eprintln!(
                "[{track:7}] peak={peak:.4} rms={rms:.5} -> {} (gain {gain:.1}x)",
                if decision {
                    "TRANSCRIBE"
                } else {
                    "GATED (silence — no ASR, no hallucination)"
                }
            );
        }
    }

    #[test]
    fn normalizes_quiet_wav_for_asr_without_changing_source() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-asr-normalize-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("mic.wav");
        let output = dir.join("mic.asr.wav");
        write_test_wav(&input, &[100, -200, 0]).unwrap();

        normalize_wav_for_asr(&input, &output, 64.0).unwrap();

        assert_eq!(read_test_wav_samples(&input).unwrap(), vec![100, -200, 0]);
        assert_eq!(
            read_test_wav_samples(&output).unwrap(),
            vec![6_400, -12_800, 0]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn audio_writer_finalizes_after_stop_even_if_sender_is_still_alive() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-audio-writer-stop-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mic.wav");
        let writer = create_audio_wav_writer(&path, "mic").unwrap();
        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(4);
        tx.send(vec![100, -200, 300]).unwrap();
        let stop = Arc::new(AtomicBool::new(true));

        let summary = write_audio_wav(
            &path,
            writer,
            rx,
            SAMPLE_RATE,
            Arc::new(AtomicU64::new(0)),
            "mic",
            stop,
        )
        .unwrap();

        assert_eq!(summary.samples_written, 3);
        assert_eq!(read_test_wav_samples(&path).unwrap(), vec![100, -200, 300]);
        drop(tx);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn audio_writer_finalizes_after_stop_while_audio_keeps_arriving() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-audio-writer-busy-stop-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mic.wav");
        let writer = create_audio_wav_writer(&path, "mic").unwrap();
        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(128);
        let stop = Arc::new(AtomicBool::new(false));
        let producer_done = Arc::new(AtomicBool::new(false));

        let writer_path = path.clone();
        let writer_stop = Arc::clone(&stop);
        let (done_tx, done_rx) = mpsc::channel();
        let writer_thread = thread::spawn(move || {
            let result = write_audio_wav(
                &writer_path,
                writer,
                rx,
                SAMPLE_RATE,
                Arc::new(AtomicU64::new(0)),
                "mic",
                writer_stop,
            );
            let _ = done_tx.send(result);
        });

        let producer_done_for_thread = Arc::clone(&producer_done);
        let producer_thread = thread::spawn(move || {
            while !producer_done_for_thread.load(Ordering::SeqCst) {
                match tx.try_send(vec![100, -200, 300, -400]) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        });

        thread::sleep(Duration::from_millis(25));
        stop.store(true, Ordering::SeqCst);
        let result = match done_rx.recv_timeout(Duration::from_millis(750)) {
            Ok(result) => result,
            Err(e) => {
                producer_done.store(true, Ordering::SeqCst);
                let _ = producer_thread.join();
                panic!("audio writer did not finalize while audio kept arriving: {e}");
            }
        };

        producer_done.store(true, Ordering::SeqCst);
        let _ = producer_thread.join();
        let _ = writer_thread.join();

        let summary = result.unwrap();
        assert!(summary.samples_written > 0);
        assert!(!read_test_wav_samples(&path).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn source_activity_segments_group_by_dominant_track() {
        let frames = vec![
            SourceActivityFrame {
                start_sample: 0,
                end_sample: 1600,
                mic_rms: 0.20,
                system_rms: 0.00,
            },
            SourceActivityFrame {
                start_sample: 1600,
                end_sample: 3200,
                mic_rms: 0.19,
                system_rms: 0.00,
            },
            SourceActivityFrame {
                start_sample: 3200,
                end_sample: 4800,
                mic_rms: 0.00,
                system_rms: 0.25,
            },
        ];

        let segments = source_activity_segments(&frames);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source, "local_mic");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 200);
        assert_eq!(segments[1].source, "system_audio");
        assert_eq!(segments[1].start_ms, 200);
        assert_eq!(segments[1].end_ms, 300);
    }

    #[test]
    fn formats_meeting_timeline_with_track_labels() {
        let transcript = format_meeting_timeline_transcript(&[
            MeetingTranscriptSegment {
                source: "mic".to_string(),
                speaker_id: "you".to_string(),
                speaker_name: "You".to_string(),
                start_ms: 1_000,
                end_ms: 2_000,
                text: "I am speaking.".to_string(),
            },
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 1_500,
                end_ms: 2_500,
                text: "Remote person speaking.".to_string(),
            },
        ]);

        assert_eq!(
            transcript,
            "[00:01 You] I am speaking.\n[00:01 Speaker 1] Remote person speaking."
        );
    }

    #[test]
    fn speaker_name_map_updates_segments_and_transcript_labels() {
        let mut segments = vec![
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 1_000,
                end_ms: 2_000,
                text: "Rahul, can you check the deployment?".to_string(),
            },
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 2_000,
                end_ms: 3_000,
                text: "Yes, I will check it.".to_string(),
            },
        ];
        let mut names = std::collections::HashMap::new();
        names.insert("speaker_1".to_string(), "Rahul".to_string());

        let replacements = apply_speaker_name_map(&mut segments, &names);
        let transcript = format_meeting_timeline_transcript(&segments);
        let cleaned = rewrite_speaker_labels_in_text(
            "[00:01 Speaker 1] Rahul, can you check?\nSpeaker 1: Yes.",
            &replacements,
        );

        assert_eq!(
            replacements,
            vec![("Speaker 1".to_string(), "Rahul".to_string())]
        );
        assert_eq!(segments[0].speaker_name, "Rahul");
        assert!(transcript.contains("[00:01 Rahul]"));
        assert_eq!(cleaned, "[00:01 Rahul] Rahul, can you check?\nRahul: Yes.");
    }

    #[test]
    fn inferred_speaker_name_sanitizer_rejects_generic_or_unsafe_labels() {
        assert_eq!(
            sanitize_inferred_speaker_name("Rahul Suman"),
            Some("Rahul Suman".to_string())
        );
        assert_eq!(sanitize_inferred_speaker_name("Speaker 1"), None);
        assert_eq!(sanitize_inferred_speaker_name("Host"), None);
        assert_eq!(sanitize_inferred_speaker_name("Unknown"), None);
        assert_eq!(
            sanitize_inferred_speaker_name("Rahul from engineering team today"),
            None
        );
        assert_eq!(sanitize_inferred_speaker_name("Rahul [admin]"), None);
    }

    #[test]
    fn suppresses_exact_mic_echo_duplicate() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-echo-dedupe-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source_activity_path = dir.join("meeting.source-activity.json");
        write_source_activity_for_test(
            &source_activity_path,
            vec![source_activity_for_test("system_audio", 0, 4_000)],
        );

        let filtered = suppress_mic_echo_segments(
            vec![
                system_segment_for_test(
                    1_000,
                    3_000,
                    "What would you tell a man who can scroll for hours?",
                ),
                mic_segment_for_test(
                    1_200,
                    3_200,
                    "What would you tell a man who can scroll for hours?",
                ),
            ],
            Some(&source_activity_path),
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].source, "system");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_local_mic_segment_even_when_text_matches_system() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-echo-local-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source_activity_path = dir.join("meeting.source-activity.json");
        write_source_activity_for_test(
            &source_activity_path,
            vec![source_activity_for_test("local_mic", 1_000, 4_000)],
        );

        let filtered = suppress_mic_echo_segments(
            vec![
                system_segment_for_test(1_000, 3_000, "Let's pause here."),
                mic_segment_for_test(1_100, 3_100, "Let's pause here."),
            ],
            Some(&source_activity_path),
        );

        assert_eq!(filtered.len(), 2);
        assert_eq!(
            filtered
                .iter()
                .filter(|segment| is_mic_transcript_segment(segment))
                .count(),
            1
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn system_dominant_video_mode_drops_paraphrased_mic_bleed() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-video-echo-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source_activity_path = dir.join("meeting.source-activity.json");
        write_source_activity_for_test(
            &source_activity_path,
            vec![
                source_activity_for_test("system_audio", 0, 120_000),
                source_activity_for_test("local_mic", 120_000, 123_000),
            ],
        );

        let filtered = suppress_mic_echo_segments(
            vec![
                system_segment_for_test(
                    10_000,
                    13_000,
                    "Your brain is trained for instant rewards.",
                ),
                mic_segment_for_test(10_400, 13_400, "Your brain is trained for instant rewards."),
                system_segment_for_test(
                    20_000,
                    23_000,
                    "The problem is not laziness, it is dopamine.",
                ),
                mic_segment_for_test(
                    20_500,
                    23_500,
                    "The problem is not laziness, it is dopamine.",
                ),
                system_segment_for_test(30_000, 33_000, "Focus feels difficult today."),
                mic_segment_for_test(30_500, 33_500, "This is why focus feels impossible today."),
                system_segment_for_test(121_000, 122_000, "Remote audio is still playing."),
                mic_segment_for_test(121_200, 122_000, "I need to pause."),
            ],
            Some(&source_activity_path),
        );

        let mic_texts = filtered
            .iter()
            .filter(|segment| is_mic_transcript_segment(segment))
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(mic_texts, vec!["I need to pause."]);
        assert_eq!(
            filtered
                .iter()
                .filter(|segment| is_system_transcript_segment(segment))
                .count(),
            4
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore]
    fn echo_dedupe_demo_on_real_meeting_artifacts() {
        let dir = PathBuf::from(
            std::env::var("AIRNOTE_ECHO_DEDUPE_DEMO_DIR")
                .expect("set AIRNOTE_ECHO_DEDUPE_DEMO_DIR to a saved meeting artifact folder"),
        );
        let transcript_path = dir.join("meeting.transcript.json");
        let artifact: MeetingTranscriptArtifact =
            serde_json::from_slice(&fs::read(&transcript_path).unwrap()).unwrap();
        let before_mic = artifact
            .segments
            .iter()
            .filter(|segment| is_mic_transcript_segment(segment))
            .count();
        let before_system = artifact
            .segments
            .iter()
            .filter(|segment| is_system_transcript_segment(segment))
            .count();

        let filtered = suppress_mic_echo_segments(
            artifact.segments,
            Some(&dir.join("meeting.source-activity.json")),
        );
        let after_mic = filtered
            .iter()
            .filter(|segment| is_mic_transcript_segment(segment))
            .count();
        let after_system = filtered
            .iter()
            .filter(|segment| is_system_transcript_segment(segment))
            .count();

        eprintln!(
            "echo_dedupe_demo: before_mic={before_mic} after_mic={after_mic} before_system={before_system} after_system={after_system} before_total={} after_total={}",
            before_mic + before_system,
            filtered.len()
        );
        assert_eq!(after_system, before_system);
        assert!(after_mic < before_mic);
        assert!(after_mic * 2 <= before_mic);
    }

    #[test]
    fn drops_whisper_non_speech_artifacts_before_speaker_labels() {
        let done = WhisperTranscriptionDone {
            transcript: "[BLANK_AUDIO]\nReal speech.\n[FOREIGN]".to_string(),
            latency_ms: 1,
            segments: vec![
                RawTranscriptSegment {
                    start_ms: 0,
                    end_ms: 1_000,
                    text: "[BLANK_AUDIO]".to_string(),
                },
                RawTranscriptSegment {
                    start_ms: 1_000,
                    end_ms: 2_000,
                    text: "Real speech.".to_string(),
                },
                RawTranscriptSegment {
                    start_ms: 2_000,
                    end_ms: 3_000,
                    text: "[FOREIGN]".to_string(),
                },
            ],
        };

        let filtered_text = filter_non_speech_transcript_lines(&done.transcript);
        let segments = label_transcript_segments(&done, "system", "speaker_1", "Speaker 1", 3_000);

        assert_eq!(filtered_text, "Real speech.");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_name, "Speaker 1");
        assert_eq!(segments[0].text, "Real speech.");
    }

    #[test]
    fn drops_stage_direction_and_punctuation_artifacts() {
        let filtered =
            filter_non_speech_transcript_lines("*Message*\nSo this is the box here.\n-\n*noise*");

        assert_eq!(filtered, "So this is the box here.");
    }

    #[test]
    fn detects_repetitive_whisper_hallucination_loops() {
        let bad = "I do not know what happened ".repeat(8);

        assert!(is_low_quality_transcript_artifact(&bad));
        assert!(!is_low_quality_transcript_artifact(
            "We checked the workspace option, selected the file, and confirmed the next step."
        ));
    }

    #[test]
    fn drops_low_confidence_whisper_json_segments() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-confidence-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let paths = transcript_paths_for_stem(&dir, "mic");
        let json = serde_json::json!({
            "transcription": [
                {
                    "offsets": { "from": 0, "to": 1000 },
                    "text": "This video was recorded during the COVID-19 pandemic.",
                    "tokens": [
                        { "text": "This", "p": 0.22 },
                        { "text": " video", "p": 0.30 },
                        { "text": " was", "p": 0.28 }
                    ]
                },
                {
                    "offsets": { "from": 1000, "to": 2200 },
                    "text": "Open the workspace option.",
                    "tokens": [
                        { "text": "Open", "p": 0.82 },
                        { "text": " the", "p": 0.90 },
                        { "text": " workspace", "p": 0.86 },
                        { "text": " option", "p": 0.88 }
                    ]
                }
            ]
        });
        fs::write(&paths.whisper_json, json.to_string()).unwrap();
        let config = WhisperCppConfig {
            binary: PathBuf::from("whisper-cli"),
            model: PathBuf::from("model.bin"),
            language: "en".to_string(),
            max_context_tokens: 0,
            prompt: None,
            suppress_non_speech: true,
            no_fallback: true,
            no_speech_threshold: Some(DEFAULT_WHISPER_NO_SPEECH_THRESHOLD),
            logprob_threshold: Some(DEFAULT_WHISPER_LOGPROB_THRESHOLD),
            entropy_threshold: Some(DEFAULT_WHISPER_ENTROPY_THRESHOLD),
            min_segment_confidence: Some(DEFAULT_WHISPER_MIN_SEGMENT_CONFIDENCE),
            vad_model: None,
            vad_threshold: DEFAULT_VAD_THRESHOLD,
            vad_speech_pad_ms: DEFAULT_VAD_SPEECH_PAD_MS,
            vad_min_silence_ms: DEFAULT_VAD_MIN_SILENCE_MS,
            romanize: true,
        };

        let segments = whisper_segments_from_json(&paths, 3_000, &config).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Open the workspace option.");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn drops_high_confidence_segment_inside_raw_repeated_run() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-repeated-json-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let paths = transcript_paths_for_stem(&dir, "mic");
        let json = serde_json::json!({
            "transcription": [
                {
                    "offsets": { "from": 0, "to": 1000 },
                    "text": "You want to send it to all of you?",
                    "tokens": [{ "text": "You", "p": 0.40 }]
                },
                {
                    "offsets": { "from": 1000, "to": 2000 },
                    "text": "Do you want to send it to all of you?",
                    "tokens": [{ "text": "Do", "p": 0.95 }, { "text": " you", "p": 0.95 }]
                },
                {
                    "offsets": { "from": 2000, "to": 3000 },
                    "text": "You want to send it to all of you.",
                    "tokens": [{ "text": "You", "p": 0.42 }]
                }
            ]
        });
        fs::write(&paths.whisper_json, json.to_string()).unwrap();
        let config = WhisperCppConfig {
            binary: PathBuf::from("whisper-cli"),
            model: PathBuf::from("model.bin"),
            language: "en".to_string(),
            max_context_tokens: 0,
            prompt: None,
            suppress_non_speech: true,
            no_fallback: true,
            no_speech_threshold: Some(DEFAULT_WHISPER_NO_SPEECH_THRESHOLD),
            logprob_threshold: Some(DEFAULT_WHISPER_LOGPROB_THRESHOLD),
            entropy_threshold: Some(DEFAULT_WHISPER_ENTROPY_THRESHOLD),
            min_segment_confidence: Some(DEFAULT_WHISPER_MIN_SEGMENT_CONFIDENCE),
            vad_model: None,
            vad_threshold: DEFAULT_VAD_THRESHOLD,
            vad_speech_pad_ms: DEFAULT_VAD_SPEECH_PAD_MS,
            vad_min_silence_ms: DEFAULT_VAD_MIN_SILENCE_MS,
            romanize: true,
        };

        let segments = whisper_segments_from_json(&paths, 3_000, &config).unwrap();

        assert!(segments.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn suppresses_repeated_whisper_segment_runs() {
        let segments = vec![
            RawTranscriptSegment {
                start_ms: 0,
                end_ms: 1_000,
                text: "You want to send it to all of you?".to_string(),
            },
            RawTranscriptSegment {
                start_ms: 1_000,
                end_ms: 2_000,
                text: "Do you want to send it to all of you?".to_string(),
            },
            RawTranscriptSegment {
                start_ms: 2_000,
                end_ms: 3_000,
                text: "You want to send it to all of you.".to_string(),
            },
            RawTranscriptSegment {
                start_ms: 3_000,
                end_ms: 4_000,
                text: "Open the workspace option.".to_string(),
            },
        ];

        let filtered = suppress_repeated_whisper_segment_runs(segments);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].text, "Open the workspace option.");
    }

    #[test]
    fn transcript_artifact_writes_legacy_source_diarization_json() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-diarization-artifact-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let wav_path = dir.join("meeting.merged.wav");
        write_test_wav(&wav_path, &[1_000, -1_000]).unwrap();
        let summary = MicCaptureSummary {
            path: wav_path.clone(),
            samples_written: 2,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 1,
            peak: 0.03,
        };
        let paths = transcript_paths_for_stem(&dir, "meeting");
        let segments = vec![
            MeetingTranscriptSegment {
                source: "mic".to_string(),
                speaker_id: "you".to_string(),
                speaker_name: "You".to_string(),
                start_ms: 0,
                end_ms: 500,
                text: "Hello.".to_string(),
            },
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 500,
                end_ms: 1_000,
                text: "Hi.".to_string(),
            },
        ];

        write_transcript_artifact(
            &paths,
            &summary,
            "completed",
            None,
            "en",
            "[00:00 You] Hello.\n[00:00 Speaker 1] Hi.",
            Some(10),
            None,
            MeetingCleanupSnapshot::idle(),
            segments,
            vec![dir.join("mic.wav"), dir.join("system.wav")],
            None,
        );

        let transcript_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths.json).unwrap()).unwrap();
        let diarization_path = dir.join("meeting.diarization.json");
        let diarization_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&diarization_path).unwrap()).unwrap();

        assert_eq!(
            transcript_json["diarization_json_path"].as_str().unwrap(),
            diarization_path.to_string_lossy()
        );
        assert!(transcript_json["final_diarization_json_path"].is_null());
        assert!(transcript_json["final_transcript_json_path"].is_null());
        assert_eq!(transcript_json["segments"][0]["source"], "mic");
        assert_eq!(transcript_json["segments"][0]["speaker_name"], "You");
        assert_eq!(transcript_json["segments"][1]["source"], "system");
        assert_eq!(transcript_json["segments"][1]["speaker_name"], "Speaker 1");
        assert_eq!(diarization_json["method"], "source_track_v1");
        assert_eq!(diarization_json["speakers"][0]["speaker_name"], "You");
        assert_eq!(diarization_json["speakers"][1]["speaker_name"], "Speaker 1");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn derives_final_diarization_paths_from_transcript_path() {
        let paths = transcript_paths_for_stem(Path::new("/tmp/airnote-meeting"), "meeting");

        let final_paths = final_diarization_paths_for_transcript(&paths).unwrap();

        assert!(
            final_paths
                .diarization_json
                .ends_with("meeting.diarization.final.json")
        );
        assert!(
            final_paths
                .transcript_json
                .ends_with("meeting.transcript.final.json")
        );
    }

    #[test]
    fn loads_final_transcript_text_from_completed_artifact() {
        let dir = std::env::temp_dir().join(format!("airnote-final-transcript-load-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let transcript_json = dir.join("meeting.transcript.final.json");
        fs::write(
            &transcript_json,
            r#"{"status":"completed","transcript":"[00:00 Local Speaker 1] Hello."}"#,
        )
        .unwrap();
        let paths = FinalDiarizationPaths {
            diarization_json: dir.join("meeting.diarization.final.json"),
            transcript_json,
        };
        let snapshot = MeetingFinalDiarizationSnapshot::completed(
            "nemo_sortformer_v2.1".to_string(),
            42,
            &paths,
        );

        let text = load_final_transcript_text(&snapshot).unwrap().unwrap();
        assert_eq!(text, "[00:00 Local Speaker 1] Hello.");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn light_diarization_assigns_speakers_by_overlap() {
        let segments = vec![
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 1_000,
                end_ms: 3_000,
                text: "First remote speaker.".to_string(),
            },
            MeetingTranscriptSegment {
                source: "system".to_string(),
                speaker_id: "speaker_1".to_string(),
                speaker_name: "Speaker 1".to_string(),
                start_ms: 6_000,
                end_ms: 7_500,
                text: "Second remote speaker.".to_string(),
            },
            MeetingTranscriptSegment {
                source: "mic".to_string(),
                speaker_id: "you".to_string(),
                speaker_name: "You".to_string(),
                start_ms: 8_000,
                end_ms: 9_000,
                text: "Local user stays local.".to_string(),
            },
        ];
        let turns = vec![
            LightDiarizationTurn {
                speaker_key: "remote-a".to_string(),
                start_ms: 0,
                end_ms: 5_000,
                confidence: 0.9,
            },
            LightDiarizationTurn {
                speaker_key: "remote-b".to_string(),
                start_ms: 5_000,
                end_ms: 8_000,
                confidence: 0.9,
            },
        ];

        let assigned = assign_light_diarization_to_transcript(&segments, &turns);

        assert_eq!(assigned[0].speaker_name, "Speaker 1");
        assert_eq!(assigned[1].speaker_name, "Speaker 2");
        assert_eq!(assigned[2].speaker_name, "You");
        assert_eq!(assigned[2].speaker_id, "you");
    }

    /// Runs the real light sherpa-onnx speaker detector against a copied local
    /// meeting fixture. Ignored by default because it depends on downloaded
    /// model files and local meeting data. Run with:
    ///   AIRNOTE_TEST_LIGHT_DIARIZATION_MEETING_DIR="$HOME/Library/Application Support/VoicePolish/meetings/local-1781715448267-4" \
    ///   cargo test -p said-desktop light_diarization_runs_on_real_meeting_fixture -- --ignored --nocapture
    #[test]
    #[ignore]
    fn light_diarization_runs_on_real_meeting_fixture() {
        let Some(source_dir) =
            std::env::var_os("AIRNOTE_TEST_LIGHT_DIARIZATION_MEETING_DIR").map(PathBuf::from)
        else {
            eprintln!("set AIRNOTE_TEST_LIGHT_DIARIZATION_MEETING_DIR to a meeting folder");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "airnote-light-diarization-fixture-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::copy(
            source_dir.join("meeting.merged.wav"),
            dir.join("meeting.merged.wav"),
        )
        .unwrap();
        fs::copy(
            source_dir.join("meeting.transcript.json"),
            dir.join("meeting.transcript.json"),
        )
        .unwrap();

        let paths = transcript_paths_for_stem(&dir, "meeting");
        let final_paths = final_diarization_paths_for_transcript(&paths).unwrap();
        let started = Instant::now();
        run_light_final_diarization(&dir.join("meeting.merged.wav"), &paths, &final_paths).unwrap();
        let elapsed = started.elapsed();
        let final_artifact =
            read_meeting_transcript_artifact(&final_paths.transcript_json).unwrap();
        let speaker_count = final_artifact
            .segments
            .iter()
            .map(|segment| segment.speaker_id.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();
        eprintln!(
            "light diarization fixture: {} segments, {} speakers, {:.2?}",
            final_artifact.segments.len(),
            speaker_count,
            elapsed
        );

        assert!(!final_artifact.segments.is_empty());
        assert!(speaker_count >= 1);
        assert!(final_paths.diarization_json.is_file());
        assert!(final_paths.transcript_json.is_file());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_cached_bench_meeting_intelligence_artifact() {
        let value = serde_json::json!({
            "provider": "deepseek",
            "model": "deepseek-v4-pro",
            "draft_latency_ms": 1200,
            "verify_latency_ms": 800,
            "filtered_mom": {
                "summary": "**Summary**\nThe team connected Nora to Gmail and Basecamp.\n\n**Next Steps**\n- Aaron shares credentials.",
                "action_items": [
                    {
                        "title": "Share Basecamp credentials",
                        "assignee": "Aaron",
                        "due": null,
                        "evidence": "share us the credentials of basecamp"
                    }
                ],
                "decisions": [
                    {
                        "text": "Use Basecamp for actionable email tasks.",
                        "evidence": "we will implement exactly this"
                    }
                ]
            }
        });

        let result = parse_cached_meeting_intelligence_value(&value)
            .unwrap()
            .unwrap();

        assert_eq!(result.status, "completed");
        assert_eq!(result.provider, "deepseek");
        assert_eq!(result.model, "deepseek-v4-pro");
        assert_eq!(result.latency_ms, 2000);
        assert_eq!(result.transcript_source, "cached-final");
        assert!(result.summary.contains("Basecamp"));
        assert_eq!(result.action_items.len(), 1);
        assert_eq!(result.action_items[0].assignee.as_deref(), Some("Aaron"));
        assert_eq!(result.decisions.len(), 1);
    }

    #[test]
    fn loads_cached_meeting_intelligence_from_manual_dir() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-ai-cache-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let manual_dir = dir.join("meeting-ai-manual");
        fs::create_dir_all(&manual_dir).unwrap();
        fs::write(
            manual_dir.join("latest.meeting-ai.json"),
            r#"{
              "provider": "deepseek",
              "model": "deepseek-v4-pro",
              "draft_latency_ms": 10,
              "verify_latency_ms": 20,
              "filtered_mom": {
                "summary": "Summary\nA useful meeting note.",
                "action_items": [],
                "decisions": []
              }
            }"#,
        )
        .unwrap();

        let result = load_cached_meeting_intelligence_from_dir(&dir)
            .unwrap()
            .unwrap();

        assert_eq!(result.provider, "deepseek");
        assert_eq!(result.latency_ms, 30);
        assert!(result.summary.contains("useful meeting note"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn loads_cached_meeting_artifacts_with_audio_and_segments() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-artifacts-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let audio_path = dir.join("meeting.merged.wav");
        write_test_wav(&audio_path, &[1_000, 1_000, 0, 0]).unwrap();
        fs::write(
            dir.join("meeting.transcript.final.json"),
            serde_json::json!({
                "status": "completed",
                "provider": "sortformer",
                "transcript": "[00:02 Speaker 1] Hello from the meeting.",
                "segments": [
                    {
                        "source": "system",
                        "speaker_id": "speaker_1",
                        "speaker_name": "Speaker 1",
                        "display_start_ms": 2100,
                        "display_end_ms": 3800,
                        "text": "Hello from the meeting."
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let artifacts = load_cached_meeting_artifacts_from_dir(Some("meeting-id"), &dir)
            .unwrap()
            .unwrap();

        assert_eq!(artifacts.transcript_source, "final");
        assert!(
            artifacts
                .audio_path
                .unwrap()
                .ends_with("meeting.merged.wav")
        );
        assert_eq!(artifacts.segments.len(), 1);
        assert_eq!(artifacts.segments[0].start_ms, 2100);
        assert_eq!(artifacts.segments[0].speaker_name, "Speaker 1");
        assert!(artifacts.transcript.contains("Hello from the meeting"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn loads_recovered_meeting_audio_without_merged_wav() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-recovered-meeting-artifacts-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mic_path = dir.join("mic.wav");
        let system_path = dir.join("system.wav");
        write_test_wav(&mic_path, &[1_000, 2_000, 0, 0]).unwrap();
        write_test_wav(&system_path, &[0, 500, 1_000, 0]).unwrap();
        fs::write(
            dir.join("meeting.transcript.json"),
            serde_json::json!({
                "status": "completed",
                "provider": "whisper.cpp",
                "source_wav": mic_path,
                "source_wavs": [mic_path, system_path],
                "transcript": "[00:00 You] Recovered audio is playable.",
                "segments": [
                    {
                        "source": "mic",
                        "speaker_id": "you",
                        "speaker_name": "You",
                        "start_ms": 0,
                        "end_ms": 1000,
                        "text": "Recovered audio is playable."
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let artifacts = load_cached_meeting_artifacts_from_dir(Some("meeting-id"), &dir)
            .unwrap()
            .unwrap();

        assert!(artifacts.audio_path.unwrap().ends_with("mic.wav"));
        assert!(artifacts.audio_duration_ms.is_some());
        assert_eq!(artifacts.transcript_source, "raw");
        assert_eq!(artifacts.segments.len(), 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn merges_mic_and_system_wavs_into_meeting_audio() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-merge-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mic_path = dir.join("mic.wav");
        let system_path = dir.join("system.wav");
        write_test_wav(&mic_path, &[1_000, 2_000, 0, 0]).unwrap();
        write_test_wav(&system_path, &[0, 500, 1_000, 1_500, 2_000]).unwrap();

        let session = MeetingSession {
            session_id: "test-session".to_string(),
            started_at_ms: now_ms(),
            artifact_dir: dir.clone(),
            mic_wav_path: mic_path.clone(),
            system_wav_path: system_path.clone(),
        };
        let mic = MicCaptureSummary {
            path: mic_path,
            samples_written: 4,
            dropped_chunks: 1,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.0,
        };
        let system = MicCaptureSummary {
            path: system_path,
            samples_written: 5,
            dropped_chunks: 2,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.0,
        };

        let merged = merge_meeting_audio(&session, &mic, &system).unwrap();

        assert!(merged.summary.path.ends_with("meeting.merged.wav"));
        assert!(
            merged
                .source_activity_path
                .ends_with("meeting.source-activity.json")
        );
        assert_eq!(merged.summary.samples_written, 5);
        assert_eq!(merged.summary.dropped_chunks, 3);
        let samples = read_test_wav_samples(&merged.summary.path).unwrap();
        assert_eq!(samples, vec![1_000, 2_500, 1_000, 1_500, 2_000]);
        assert!(dir.join("meeting.audio.json").is_file());
        assert!(dir.join("meeting.source-activity.json").is_file());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_transcription_builds_dual_track_timeline_plan_after_merge() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-primary-transcript-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mic_path = dir.join("mic.wav");
        let system_path = dir.join("system.wav");
        write_test_wav(&mic_path, &[1_000, 2_000, 0, 0]).unwrap();
        write_test_wav(&system_path, &[0, 500, 1_000, 1_500]).unwrap();

        let state = MeetingEngineState::new();
        let session = MeetingSession {
            session_id: "test-session".to_string(),
            started_at_ms: now_ms(),
            artifact_dir: dir.clone(),
            mic_wav_path: mic_path.clone(),
            system_wav_path: system_path.clone(),
        };
        let mic = MicCaptureSummary {
            path: mic_path.clone(),
            samples_written: 4,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.1,
        };
        let system = MicCaptureSummary {
            path: system_path.clone(),
            samples_written: 4,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.1,
        };

        let plan = state
            .prepare_transcription_source(Some(&session), Some(mic), Some(system))
            .unwrap();
        let audio = state.audio.lock_recover().clone();

        assert_eq!(plan.mic.path, mic_path);
        assert_eq!(
            plan.system.as_ref().map(|summary| &summary.path),
            Some(&system_path)
        );
        assert!(plan.summary.path.ends_with("meeting.merged.wav"));
        assert!(plan.output_paths.text.ends_with("meeting.transcript.txt"));
        assert_eq!(plan.source_wavs, vec![mic_path, system_path]);
        assert!(
            plan.source_activity_path
                .as_ref()
                .is_some_and(|path| path.ends_with("meeting.source-activity.json"))
        );
        assert_eq!(audio.status, "completed");
        assert!(
            audio
                .merged_path
                .as_ref()
                .is_some_and(|path| path.ends_with("meeting.merged.wav"))
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_transcription_skips_silent_system_track() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-silent-system-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mic_path = dir.join("mic.wav");
        let system_path = dir.join("system.wav");
        write_test_wav(&mic_path, &[1_000, 2_000, 0, 0]).unwrap();
        write_test_wav(&system_path, &[0, 0, 0, 0]).unwrap();

        let state = MeetingEngineState::new();
        let session = MeetingSession {
            session_id: "test-session".to_string(),
            started_at_ms: now_ms(),
            artifact_dir: dir.clone(),
            mic_wav_path: mic_path.clone(),
            system_wav_path: system_path.clone(),
        };
        let mic = MicCaptureSummary {
            path: mic_path.clone(),
            samples_written: 4,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.1,
        };
        let system = MicCaptureSummary {
            path: system_path,
            samples_written: 4,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 0,
            peak: 0.0,
        };

        let plan = state
            .prepare_transcription_source(Some(&session), Some(mic), Some(system))
            .unwrap();
        let audio = state.audio.lock_recover().clone();

        assert_eq!(plan.system.as_ref().map(|summary| &summary.path), None);
        assert!(plan.output_paths.text.ends_with("meeting.transcript.txt"));
        assert_eq!(plan.source_wavs, vec![mic_path]);
        assert!(plan.source_activity_path.is_none());
        assert_eq!(audio.status, "skipped_silent_system_audio");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn build_retranscribe_plan_carries_source_activity_path() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-retranscribe-plan-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_test_wav(&dir.join("mic.wav"), &[1_000, 2_000, 0, 0]).unwrap();
        write_test_wav(&dir.join("system.wav"), &[0, 500, 1_000, 1_500]).unwrap();
        write_test_wav(
            &dir.join("meeting.merged.wav"),
            &[1_000, 2_500, 1_000, 1_500],
        )
        .unwrap();
        write_source_activity_for_test(
            &dir.join("meeting.source-activity.json"),
            vec![source_activity_for_test("system_audio", 0, 1_000)],
        );

        let plan = build_retranscribe_plan(&dir).unwrap();

        assert_eq!(
            plan.source_wavs,
            vec![dir.join("mic.wav"), dir.join("system.wav")]
        );
        assert!(
            plan.source_activity_path
                .as_ref()
                .is_some_and(|path| path.ends_with("meeting.source-activity.json"))
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn meeting_whisper_defaults_prefer_q5_for_light_meeting_flow() {
        let candidates = default_whisper_model_candidates(Path::new("/tmp/airnote-test-data"));

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].ends_with("models/ggml-large-v3-turbo-q5_0.bin"));
    }

    #[test]
    fn meeting_whisper_model_dir_chooses_recommended_low_memory_model() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-model-dir-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        // Real-sized stubs: the resolver skips empty/partial models (< 1 MB).
        let blob = vec![0u8; (MIN_WHISPER_MODEL_BYTES + 1) as usize];
        fs::write(dir.join("ggml-large-v3-turbo-q5_0.bin"), &blob).unwrap();
        fs::write(dir.join("ggml-large-v3-turbo.bin"), &blob).unwrap();

        let selected = first_whisper_model_in_dir(&dir).unwrap();

        assert!(selected.ends_with("ggml-large-v3-turbo-q5_0.bin"));

        // A 0-byte placeholder must be ignored, not chosen.
        let empty_dir = dir.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        fs::write(empty_dir.join("ggml-large-v3-turbo.bin"), b"").unwrap();
        assert!(first_whisper_model_in_dir(&empty_dir).is_none());

        let legacy_only_dir = dir.join("legacy-only");
        fs::create_dir_all(&legacy_only_dir).unwrap();
        fs::write(legacy_only_dir.join("ggml-large-v3-turbo.bin"), &blob).unwrap();
        assert!(first_whisper_model_in_dir(&legacy_only_dir).is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn meeting_whisper_model_selection_does_not_require_binary() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-active-model-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let blob = vec![0u8; (MIN_WHISPER_MODEL_BYTES + 1) as usize];
        let model = dir.join("ggml-large-v3-turbo-q5_0.bin");
        fs::write(&model, &blob).unwrap();

        let selected = choose_whisper_model_path(Some(model.clone()), None).unwrap();

        assert_eq!(selected, model);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn meeting_whisper_catalog_exposes_supported_multilingual_models() {
        let names = WHISPER_MODEL_CATALOG
            .iter()
            .map(|(name, _, _)| *name)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(names.len(), 5);
        assert!(names.contains("ggml-large-v3-turbo-q5_0.bin"));
        assert!(names.contains("ggml-medium.bin"));
        assert!(names.contains("ggml-small.bin"));
        assert!(names.contains("ggml-base.bin"));
        assert!(names.contains("ggml-tiny.bin"));
    }

    #[test]
    fn meeting_whisper_cleanup_removes_unsupported_models_and_keeps_supported() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-cleanup-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ggml-large-v3-turbo-q5_0.bin"), b"keep").unwrap();
        fs::write(dir.join("ggml-large-v3-turbo-q5_0.bin.part"), b"keep-part").unwrap();
        fs::write(dir.join("ggml-large-v3-turbo.bin"), b"delete-full").unwrap();
        fs::write(dir.join("ggml-medium.bin"), b"delete-medium").unwrap();
        fs::write(dir.join("ggml-small-q5_0.bin.part"), b"delete-part").unwrap();
        fs::write(dir.join("ggml-silero-v5.1.2.bin"), b"keep-vad").unwrap();

        let result = cleanup_legacy_whisper_models_in_dir(&dir).unwrap();
        let removed = result
            .removed
            .iter()
            .map(|model| model.name.as_str())
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(removed.len(), 2);
        assert!(removed.contains("ggml-large-v3-turbo.bin"));
        assert!(removed.contains("ggml-small-q5_0.bin.part"));
        assert!(dir.join("ggml-large-v3-turbo-q5_0.bin").exists());
        assert!(dir.join("ggml-large-v3-turbo-q5_0.bin.part").exists());
        assert!(dir.join("ggml-medium.bin").exists());
        assert!(dir.join("ggml-silero-v5.1.2.bin").exists());
        assert!(!dir.join("ggml-large-v3-turbo.bin").exists());
        assert!(!dir.join("ggml-small-q5_0.bin.part").exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn strips_llm_transcript_wrappers_without_rewriting_words() {
        let cleaned = strip_llm_transcript_wrappers(
            "```text\nCleaned transcript:\nLine one stays.\nLine two stays.\n```",
        );

        assert_eq!(cleaned, "Line one stays.\nLine two stays.");
    }

    #[test]
    fn meeting_ai_transcript_prefers_final_then_cleaned_then_raw() {
        let mut snapshot = TranscriptionSnapshot {
            text: Some("raw transcript".to_string()),
            cleaned_text: Some("cleaned transcript".to_string()),
            final_text: Some("final transcript".to_string()),
            ..TranscriptionSnapshot::default()
        };

        let selected = select_meeting_ai_transcript_from_snapshot(&snapshot).unwrap();
        assert_eq!(selected.source, "final");
        assert_eq!(selected.text, "final transcript");

        snapshot.final_text = None;
        let selected = select_meeting_ai_transcript_from_snapshot(&snapshot).unwrap();
        assert_eq!(selected.source, "cleaned");
        assert_eq!(selected.text, "cleaned transcript");

        snapshot.cleaned_text = None;
        let selected = select_meeting_ai_transcript_from_snapshot(&snapshot).unwrap();
        assert_eq!(selected.source, "raw");
        assert_eq!(selected.text, "raw transcript");
    }

    #[test]
    fn parses_meeting_intelligence_json_from_fenced_response() {
        let response = r#"```json
{
  "summary": "Speaker discussed sampling tokens.",
  "action_items": [
    { "title": "Review transcript", "assignee": "Local Speaker 1", "due": null }
  ],
  "decisions": [
    { "text": "Use final transcript for MoM." },
    "Keep raw transcript as fallback."
  ]
}
```"#;

        let (_title, _tags, summary, action_items, decisions) =
            parse_meeting_intelligence(response, None).unwrap();

        assert_eq!(summary, "Speaker discussed sampling tokens.");
        assert_eq!(action_items.len(), 1);
        assert_eq!(action_items[0].title, "Review transcript");
        assert_eq!(action_items[0].assignee.as_deref(), Some("Local Speaker 1"));
        assert!(action_items[0].evidence.is_none());
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].text, "Use final transcript for MoM.");
        assert!(decisions[0].evidence.is_none());
        assert_eq!(decisions[1].text, "Keep raw transcript as fallback.");
    }

    #[test]
    fn meeting_intelligence_filters_items_without_matching_evidence() {
        let transcript = "\
[00:01 Speaker 1] Please review the transcript before Friday.
[00:05 Speaker 2] Agreed, we will use the final transcript for MoM.
";
        let response = r#"{
  "summary": "Speakers discussed transcript review.",
  "action_items": [
    {
      "title": "Review transcript",
      "assignee": "Speaker 1",
      "due": "Friday",
      "evidence": "Please review the transcript before Friday.",
      "support": "firm"
    },
    {
      "title": "Prepare the launch plan",
      "assignee": "Speaker 2",
      "due": null,
      "evidence": "Prepare the launch plan.",
      "support": "firm"
    }
  ],
  "decisions": [
    {
      "text": "Use final transcript for MoM.",
      "evidence": "Agreed, we will use the final transcript for MoM.",
      "support": "explicit"
    },
    {
      "text": "Keep raw transcript as fallback.",
      "evidence": "Keep raw transcript as fallback.",
      "support": "explicit"
    }
  ]
}"#;

        let (_title, _tags, _summary, action_items, decisions) =
            parse_meeting_intelligence(response, Some(transcript)).unwrap();

        assert_eq!(action_items.len(), 1);
        assert_eq!(action_items[0].title, "Review transcript");
        assert_eq!(
            action_items[0].evidence.as_deref(),
            Some("Please review the transcript before Friday.")
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].text, "Use final transcript for MoM.");
        assert_eq!(
            decisions[0].evidence.as_deref(),
            Some("Agreed, we will use the final transcript for MoM.")
        );
    }

    #[test]
    fn meeting_intelligence_filters_items_without_required_support() {
        let transcript = "\
[00:01 Speaker 1] Please review the transcript before Friday.
[00:05 Speaker 2] Agreed, we will use the final transcript for MoM.
";
        let response = r#"{
  "summary": "Speakers discussed transcript review.",
  "action_items": [
    {
      "title": "Review transcript",
      "assignee": "Speaker 1",
      "due": "Friday",
      "evidence": "Please review the transcript before Friday.",
      "support": "tentative"
    }
  ],
  "decisions": [
    {
      "text": "Use final transcript for MoM.",
      "evidence": "Agreed, we will use the final transcript for MoM.",
      "support": "implicit"
    }
  ]
}"#;

        let (_title, _tags, _summary, action_items, decisions) =
            parse_meeting_intelligence(response, Some(transcript)).unwrap();

        assert!(action_items.is_empty());
        assert!(decisions.is_empty());
    }

    #[test]
    fn repairs_zero_sized_wav_header() {
        let path = std::env::temp_dir().join(format!(
            "airnote-zero-wav-header-{}-{}.wav",
            std::process::id(),
            now_ms()
        ));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 320]);
        fs::write(&path, bytes).unwrap();

        repair_wav_header_sizes(&path).unwrap();

        let repaired = fs::read(&path).unwrap();
        let riff_size = u32::from_le_bytes([repaired[4], repaired[5], repaired[6], repaired[7]]);
        let data_size =
            u32::from_le_bytes([repaired[40], repaired[41], repaired[42], repaired[43]]);
        assert_eq!(riff_size, (repaired.len() - 8) as u32);
        assert_eq!(data_size, (repaired.len() - 44) as u32);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn write_atomic_replaces_contents_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-write-atomic-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("artifact.json");

        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");

        write_atomic(&path, b"second-and-longer").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second-and-longer");

        // The sibling temp file must not survive a successful write.
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        assert!(!PathBuf::from(tmp).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn meeting_state_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-meeting-state-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();

        assert!(read_meeting_state(&dir).is_none());

        write_meeting_state(&dir, MEETING_PHASE_TRANSCRIBING, None);
        let state = read_meeting_state(&dir).expect("state should be readable");
        assert_eq!(state.phase, MEETING_PHASE_TRANSCRIBING);
        assert!(state.error.is_none());

        write_meeting_state(&dir, MEETING_PHASE_FAILED, Some("boom".to_string()));
        let state = read_meeting_state(&dir).expect("state should be readable");
        assert_eq!(state.phase, MEETING_PHASE_FAILED);
        assert_eq!(state.error.as_deref(), Some("boom"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn incomplete_completed_transcript_is_resumable_until_terminal() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-resume-transcript-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let paths = transcript_paths_for_stem(&dir, "meeting");
        let summary = MicCaptureSummary {
            path: dir.join("mic.wav"),
            samples_written: SAMPLE_RATE as u64,
            dropped_chunks: 0,
            native_rate: SAMPLE_RATE,
            duration_ms: 1_000,
            peak: 0.1,
        };

        write_meeting_state(&dir, MEETING_PHASE_TRANSCRIBING, None);
        assert!(!should_resume_incomplete_transcript(&dir));

        write_transcript_artifact(
            &paths,
            &summary,
            "completed",
            None,
            DEFAULT_WHISPER_LANGUAGE,
            "hello from the saved transcript",
            Some(123),
            Some("hello from the cleaned transcript"),
            MeetingCleanupSnapshot::skipped("skipped_test", "test"),
            Vec::new(),
            vec![summary.path.clone()],
            None,
        );
        assert!(should_resume_incomplete_transcript(&dir));

        write_meeting_state(&dir, MEETING_PHASE_SUMMARIZED, None);
        assert!(!should_resume_incomplete_transcript(&dir));

        write_meeting_state(&dir, MEETING_PHASE_FAILED, Some("boom".to_string()));
        assert!(!should_resume_incomplete_transcript(&dir));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn meeting_final_diarization_is_disabled() {
        assert!(!meeting_final_diarization_enabled());
        assert_eq!(meeting_final_diarization_mode(), FINAL_DIARIZATION_MODE_OFF);
    }

    #[test]
    fn light_diarization_skips_only_over_the_duration_limit() {
        let max_ms = 15 * 60 * 1000;

        assert!(light_diarization_skip_reason_for_duration(max_ms, max_ms).is_none());
        let reason = light_diarization_skip_reason_for_duration(max_ms + 1, max_ms)
            .expect("duration over limit should skip");
        assert!(reason.contains("light speaker detection skipped"));
        assert!(reason.contains("900.0s"));
    }

    #[test]
    fn parses_title_and_tags_and_normalizes_them() {
        let response = r##"{
  "title": "\"Stryker Sentinel Pricing & Rollout.\"",
  "tags": ["#Pricing", "Security", "pricing", "  ", "Onboarding", "Risk", "Extra", "Toomany"],
  "summary": "Discussed pricing and rollout.",
  "action_items": [],
  "decisions": []
}"##;
        let (title, tags, summary, _actions, _decisions) =
            parse_meeting_intelligence(response, None).unwrap();
        // Surrounding quotes and trailing period stripped.
        assert_eq!(title, "Stryker Sentinel Pricing & Rollout");
        // Leading '#' stripped, blanks dropped, case-insensitive de-dupe, capped at 6.
        assert_eq!(
            tags,
            vec![
                "Pricing",
                "Security",
                "Onboarding",
                "Risk",
                "Extra",
                "Toomany"
            ]
        );
        assert_eq!(summary, "Discussed pricing and rollout.");
    }

    #[test]
    fn user_tags_add_dedupes_and_remove_persists() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-user-tags-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();

        assert!(read_meeting_user_tags(&dir).is_empty());

        write_meeting_user_tags(&dir, &["Pricing".to_string()]).unwrap();
        let mut tags = read_meeting_user_tags(&dir);
        // Case-insensitive de-dupe is enforced by the command, mirror it here.
        let candidate = sanitize_user_tag("#pricing").unwrap();
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
            tags.push(candidate);
        }
        assert_eq!(tags, vec!["Pricing".to_string()]);

        write_meeting_user_tags(&dir, &["Pricing".to_string(), "Risk".to_string()]).unwrap();
        let mut tags = read_meeting_user_tags(&dir);
        tags.retain(|t| !t.eq_ignore_ascii_case("pricing"));
        write_meeting_user_tags(&dir, &tags).unwrap();
        assert_eq!(read_meeting_user_tags(&dir), vec!["Risk".to_string()]);

        let _ = fs::remove_dir_all(&dir);
    }

    fn mic_segment_for_test(start_ms: u64, end_ms: u64, text: &str) -> MeetingTranscriptSegment {
        MeetingTranscriptSegment {
            source: "mic".to_string(),
            speaker_id: "you".to_string(),
            speaker_name: "You".to_string(),
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    fn system_segment_for_test(start_ms: u64, end_ms: u64, text: &str) -> MeetingTranscriptSegment {
        MeetingTranscriptSegment {
            source: "system".to_string(),
            speaker_id: "speaker_1".to_string(),
            speaker_name: "Speaker 1".to_string(),
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    fn source_activity_for_test(source: &str, start_ms: u64, end_ms: u64) -> SourceActivitySegment {
        SourceActivitySegment {
            source: source.to_string(),
            start_ms,
            end_ms,
            mic_rms: if source == "local_mic" || source == "overlap" {
                0.20
            } else {
                0.0
            },
            system_rms: if source == "system_audio" || source == "overlap" {
                0.20
            } else {
                0.0
            },
        }
    }

    fn write_source_activity_for_test(path: &Path, segments: Vec<SourceActivitySegment>) {
        let artifact = MeetingAudioArtifact {
            schema_version: 1,
            status: "completed".to_string(),
            mic_wav: "mic.wav".to_string(),
            system_wav: "system.wav".to_string(),
            merged_wav: Some("meeting.merged.wav".to_string()),
            source_activity_path: Some(path.to_string_lossy().to_string()),
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            duration_ms: segments.iter().map(|segment| segment.end_ms).max(),
            samples_written: 0,
            source_activity_segments: segments,
            generated_at_ms: now_ms(),
            error: None,
        };
        fs::write(path, serde_json::to_vec_pretty(&artifact).unwrap()).unwrap();
    }

    fn write_test_wav(path: &Path, samples: &[i16]) -> Result<(), hound::Error> {
        let spec = hound::WavSpec {
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)?;
        for sample in samples {
            writer.write_sample(*sample)?;
        }
        writer.finalize()
    }

    fn read_test_wav_samples(path: &Path) -> Result<Vec<i16>, hound::Error> {
        hound::WavReader::open(path)?
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
    }
}
