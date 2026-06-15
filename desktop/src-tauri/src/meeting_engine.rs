use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufWriter, Read, Seek, SeekFrom, Write};
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

const STATUS_EVENT: &str = "meeting-engine-state";
const LIVE_TRANSCRIPT_EVENT: &str = "meeting-engine-live-transcript";
const PHASE: &str = "system_audio_capture";
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const AUDIO_QUEUE_DEPTH: usize = 512;
const LIVE_AUDIO_QUEUE_DEPTH: usize = 4096;
const LIVE_TRANSCRIPT_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_LIVE_TRANSCRIPT_CHUNK_SECS: u64 = 8;
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
const WHISPER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
// Near-silent tracks (e.g. a system track with no remote participant) make
// whisper hallucinate text like "Thank you" / "aaaa" on silence, so require a
// small but real signal before transcribing. -44 dB is still well below even
// whispered speech, so genuine quiet audio is kept.
const ASR_MIN_PEAK_FOR_TRANSCRIPTION: f32 = 0.006;
// Default to English: it's reliable for English/Hinglish meetings (the common
// case) and stable on short live windows. "auto" mis-detects and hallucinates
// random scripts (ja/ko/es) on near-silent mic / speaker bleed. Pure-Hindi
// meetings should pick a language explicitly (per-meeting selector / env
// AIRNOTE_MEETING_MIC_WHISPER_LANGUAGE=hi).
const DEFAULT_WHISPER_LANGUAGE: &str = "en";
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
const DEFAULT_MEETING_CLEANUP_PROVIDER: &str = "groq";
const DEFAULT_GATEWAY_MEETING_CLEANUP_MODEL: &str = "gemini-2.5-flash";
const DEFAULT_GROQ_MEETING_CLEANUP_MODEL: &str = "llama-3.3-70b-versatile";
const DEFAULT_DEEPSEEK_MEETING_CLEANUP_MODEL: &str = "deepseek-v4-pro";
const DEFAULT_MEETING_CLEANUP_TIMEOUT_SECS: u64 = 90;
const DEFAULT_MEETING_CLEANUP_MAX_TOKENS: u64 = 8192;
const DEFAULT_MEETING_AI_TIMEOUT_SECS: u64 = 120;
const DEFAULT_MEETING_AI_MAX_TOKENS: u64 = 8192;
const DEFAULT_FINAL_DIARIZATION_TIMEOUT_SECS: u64 = 30 * 60;
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
    chunk_samples: usize,
    min_samples: usize,
    poll_interval: Duration,
    timeout: Duration,
}

struct LiveTrackBuffer {
    source: LiveAudioSource,
    base_sample: u64,
    samples: Vec<i16>,
}

impl LiveTrackBuffer {
    fn new(source: LiveAudioSource) -> Self {
        Self {
            source,
            base_sample: 0,
            samples: Vec::new(),
        }
    }

    fn push(&mut self, samples: Vec<i16>) {
        self.samples.extend(samples);
    }

    fn take_ready_window(
        &mut self,
        chunk_samples: usize,
        min_samples: usize,
        force: bool,
    ) -> Option<LiveTranscriptWindow> {
        if self.samples.len() < min_samples {
            return None;
        }
        if !force && self.samples.len() < chunk_samples {
            return None;
        }

        let take = if force {
            self.samples.len()
        } else {
            self.samples.len().min(chunk_samples)
        };
        let samples: Vec<i16> = self.samples.drain(..take).collect();
        let start_sample = self.base_sample;
        self.base_sample = self.base_sample.saturating_add(take as u64);
        Some(LiveTranscriptWindow {
            source: self.source,
            start_sample,
            samples,
        })
    }
}

struct LiveTranscriptWindow {
    source: LiveAudioSource,
    start_sample: u64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeetingAudioTrack {
    Mic,
    System,
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

#[derive(Clone, Debug, Serialize)]
struct MeetingTranscriptSegment {
    source: String,
    speaker_id: String,
    speaker_name: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone, Debug, Serialize)]
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
struct SourceActivityFrame {
    start_sample: u64,
    end_sample: u64,
    mic_rms: f32,
    system_rms: f32,
}

#[derive(Clone, Debug, Serialize)]
struct SourceActivitySegment {
    source: String,
    start_ms: u64,
    end_ms: u64,
    mic_rms: f32,
    system_rms: f32,
}

#[derive(Clone, Debug, Serialize)]
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

        let mut session = self.session.lock().expect("meeting engine lock poisoned");
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
                    session = self.session.lock().expect("meeting engine lock poisoned");
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
            *self
                .last_mic_summary
                .lock()
                .expect("meeting engine lock poisoned") = None;
            *self
                .last_system_summary
                .lock()
                .expect("meeting engine lock poisoned") = None;
            *self
                .system_error
                .lock()
                .expect("meeting engine lock poisoned") = None;
            *self.audio.lock().expect("meeting engine lock poisoned") =
                MeetingAudioSnapshot::default();
            *self
                .transcription
                .lock()
                .expect("meeting engine lock poisoned") = TranscriptionSnapshot::default();
            *self
                .live_transcript
                .lock()
                .expect("meeting engine lock poisoned") = LiveTranscriptSnapshot {
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
        let session = self
            .session
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let system_summary = self.stop_system_capture();
        let mic_summary = self.stop_mic_capture();
        self.stop_live_transcript();
        let transcription_plan = self.prepare_transcription_source(
            session.as_ref(),
            mic_summary.clone(),
            system_summary,
        );
        let session_dir = session.as_ref().map(|s| s.artifact_dir.clone());
        self.active.store(false, Ordering::SeqCst);
        self.muted.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut session_guard = self.session.lock().expect("meeting engine lock poisoned");
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
        let session = self
            .session
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let mic_running = self
            .mic
            .lock()
            .expect("meeting engine lock poisoned")
            .is_some();
        let system_running = self
            .system
            .lock()
            .expect("meeting engine lock poisoned")
            .is_some();
        let summary = self
            .last_mic_summary
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let system_summary = self
            .last_system_summary
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let last_error = self
            .last_error
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let system_error = self
            .system_error
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let transcription = self
            .transcription
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let live_transcript = self
            .live_transcript
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let audio = self
            .audio
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();

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
        if self
            .live_transcript_handle
            .lock()
            .expect("meeting engine lock poisoned")
            .is_some()
        {
            return;
        }

        let session = self
            .session
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let Some(session) = session else {
            self.set_live_transcript_error("meeting session is not initialized".to_string());
            return;
        };

        if !env_bool("AIRNOTE_MEETING_LIVE_TRANSCRIPT_ENABLED", true) {
            let mut live = self
                .live_transcript
                .lock()
                .expect("meeting engine lock poisoned");
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
                *self
                    .live_transcript_handle
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(handle);
            }
            Err(e) => {
                self.set_live_transcript_error(e);
            }
        }
    }

    fn stop_live_transcript(&self) {
        let handle = self
            .live_transcript_handle
            .lock()
            .expect("meeting engine lock poisoned")
            .take();
        let Some(mut handle) = handle else {
            return;
        };

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
        self.live_transcript
            .lock()
            .expect("meeting engine lock poisoned")
            .payload()
    }

    fn live_audio_sender(&self) -> Option<mpsc::SyncSender<LiveAudioChunk>> {
        self.live_transcript_handle
            .lock()
            .expect("meeting engine lock poisoned")
            .as_ref()
            .map(|handle| handle.audio_tx.clone())
    }

    fn set_live_transcript_error(&self, error: String) {
        tracing::warn!(error = %error, "[meeting_engine] live transcript unavailable");
        let mut live = self
            .live_transcript
            .lock()
            .expect("meeting engine lock poisoned");
        live.running = false;
        live.status = "skipped".to_string();
        live.error = Some(error);
    }

    fn ensure_mic_capture(&self) {
        if self
            .mic
            .lock()
            .expect("meeting engine lock poisoned")
            .is_some()
        {
            return;
        }

        let session = self
            .session
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
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
                *self.mic.lock().expect("meeting engine lock poisoned") = Some(handle);
                self.set_last_error(None);
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] mic capture failed to start");
                self.set_last_error(Some(e));
            }
        }
    }

    fn ensure_system_capture(&self) {
        if self
            .system
            .lock()
            .expect("meeting engine lock poisoned")
            .is_some()
        {
            return;
        }

        let session = self
            .session
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();
        let Some(session) = session else {
            *self
                .system_error
                .lock()
                .expect("meeting engine lock poisoned") =
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
                *self.system.lock().expect("meeting engine lock poisoned") = Some(handle);
                *self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned") = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] system audio capture failed to start");
                *self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(e);
            }
        }
    }

    fn stop_mic_capture(&self) -> Option<MicCaptureSummary> {
        let handle = self
            .mic
            .lock()
            .expect("meeting engine lock poisoned")
            .take();
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
                *self
                    .last_mic_summary
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(summary.clone());
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
                self.set_last_error(Some(message));
                None
            }
        }
    }

    fn stop_system_capture(&self) -> Option<SystemCaptureSummary> {
        let handle = self
            .system
            .lock()
            .expect("meeting engine lock poisoned")
            .take();
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
                *self
                    .last_system_summary
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(summary.clone());
                *self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned") = None;
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                Some(summary)
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "[meeting_engine] system audio capture finalize failed");
                *self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(e);
                if let Some(join) = handle.join.take() {
                    let _ = join.join();
                }
                None
            }
            Err(e) => {
                let message = format!("timed out while stopping system audio capture: {e}");
                tracing::warn!(error = %message, "[meeting_engine] system audio capture stop timed out");
                *self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned") = Some(message);
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
            *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
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
        };

        let Some(session) = session else {
            *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
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
        };

        let Some(system_summary) = system_summary else {
            *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
                status: "skipped_missing_system_audio".to_string(),
                error: self
                    .system_error
                    .lock()
                    .expect("meeting engine lock poisoned")
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
            *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
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
                *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
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
                })
            }
            Err(e) => {
                tracing::warn!(error = %e, "[meeting_engine] meeting audio merge failed");
                *self.audio.lock().expect("meeting engine lock poisoned") = MeetingAudioSnapshot {
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
        self.jobs.shutdown.store(true, Ordering::SeqCst);
        self.jobs.cvar.notify_all();
        if self.active.load(Ordering::SeqCst) {
            tracing::info!("[meeting_engine] finalizing active recording on shutdown");
            let _ = self.stop();
        }
    }

    /// Startup recovery: re-enqueue meetings that were interrupted mid-pipeline
    /// (non-terminal phase, audio on disk, no usable transcript yet) so a crash
    /// or force-quit during transcription self-heals on next launch. `failed`
    /// meetings are left for the user's explicit Retry. Summary generation stays
    /// lazy (on open), so meetings that already have a transcript are skipped.
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
                Some(MEETING_PHASE_TRANSCRIBED | MEETING_PHASE_SUMMARIZED | MEETING_PHASE_FAILED)
            ) {
                continue;
            }
            if meeting_has_usable_transcript(&dir) {
                continue;
            }
            // Only the tracks build_retranscribe_plan can actually use — mic or
            // the merged mixdown. A system-only or audio-less orphan dir can't be
            // re-transcribed, so skip it silently instead of log-spamming a "no
            // audio" error every launch (the storage GC reclaims those dirs).
            let has_retranscribable_audio =
                dir.join("mic.wav").is_file() || dir.join("meeting.merged.wav").is_file();
            if !has_retranscribable_audio {
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
        *self
            .last_error
            .lock()
            .expect("meeting engine lock poisoned") = error;
    }

    fn emit_status(&self, app: &AppHandle) -> MeetingEngineStatus {
        let status = self.status();
        let _ = app.emit(STATUS_EVENT, status.clone());
        status
    }

    #[cfg(test)]
    fn install_fake_mic_capture_for_test(&self) {
        let (stop_tx, _stop_rx) = mpsc::channel();
        let (_done_tx, done_rx) = mpsc::channel();
        *self.mic.lock().expect("meeting engine lock poisoned") = Some(MicCaptureHandle {
            stop_tx,
            done_rx,
            join: None,
        });
    }

    #[cfg(test)]
    fn install_fake_system_capture_for_test(&self) {
        let (stop_tx, _stop_rx) = mpsc::channel();
        let (_done_tx, done_rx) = mpsc::channel();
        *self.system.lock().expect("meeting engine lock poisoned") = Some(SystemCaptureHandle {
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
        inner.pending.push_back(job);
        drop(inner);
        self.cvar.notify_one();
        EnqueueOutcome::Enqueued
    }

    /// True if a meeting is currently queued or being processed.
    fn is_active(&self, meeting_id: &str) -> bool {
        let inner = self.lock();
        inner.in_flight.as_deref() == Some(meeting_id)
            || inner.pending.iter().any(|j| j.meeting_id == meeting_id)
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
    Retry(String),
    Terminal(String),
}

/// Classify a transcription error as retryable (transient: process spawn,
/// timeout, IO, network) or terminal (no audio, missing binary/key — retrying
/// won't change the result).
fn classify_meeting_job_error(message: &str) -> JobOutcome {
    let m = message.to_ascii_lowercase();
    let terminal = m.contains("no confident speech")
        || m.contains("below speech threshold")
        || m.contains("empty")
        || m.contains("no audio")
        || m.contains("api key")
        || m.contains("_api_key")
        || m.contains("missing whisper")
        || m.contains("whisper.cpp binary")
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
                    let job = inner.pending.remove(pos).expect("position valid");
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
            run_transcription_job(&job.plan, &transcription, is_last_attempt)
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
        if let JobOutcome::Retry(msg) = outcome {
            let backoff =
                (MEETING_JOB_BACKOFF_BASE_MS << job.attempt).min(MEETING_JOB_BACKOFF_MAX_MS);
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
    plan: &MeetingTranscriptionPlan,
    transcription_state: &Arc<Mutex<TranscriptionSnapshot>>,
    is_last_attempt: bool,
) -> JobOutcome {
    let transcript_paths = plan.output_paths.clone();
    let job_artifact_dir = transcript_paths.text.parent().map(Path::to_path_buf);
    if let Some(dir) = job_artifact_dir.as_deref() {
        write_meeting_state(dir, MEETING_PHASE_TRANSCRIBING, None);
    }
    {
        let mut transcription = transcription_state
            .lock()
            .expect("meeting engine lock poisoned");
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
            let mut transcription = transcription_state
                .lock()
                .expect("meeting engine lock poisoned");
            transcription.running = false;
            transcription.status = "skipped_empty_audio".to_string();
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
                let mut transcription = transcription_state
                    .lock()
                    .expect("meeting engine lock poisoned");
                transcription.running = false;
                transcription.status = "skipped_missing_whisper".to_string();
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
        let mut transcription = transcription_state
            .lock()
            .expect("meeting engine lock poisoned");
        transcription.language = Some(config.language.clone());
        transcription.model = Some(config.model.to_string_lossy().to_string());
    }

    match transcribe_meeting_plan(plan, &config) {
        Ok(mut done) => {
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
                let mut transcription = transcription_state
                    .lock()
                    .expect("meeting engine lock poisoned");
                transcription.status = "cleaning".to_string();
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
            let (cleaned_transcript, cleanup) = match cleanup_result {
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
                done.transcript = done
                    .segments
                    .iter()
                    .map(|s| s.text.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
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

            let final_diarization = run_final_diarization_stage(
                transcription_state,
                &transcript_paths,
                &done.summary.path,
            );
            let final_transcript_text = load_final_transcript_text(&final_diarization)
                .ok()
                .flatten();

            // Capture the best transcript text for the summary stage before the
            // originals are moved into the snapshot below.
            let (summary_source, summary_text) = if let Some(text) = &final_transcript_text {
                ("final".to_string(), text.clone())
            } else if let Some(text) = &cleaned_transcript {
                ("cleaned".to_string(), text.clone())
            } else {
                ("raw".to_string(), done.transcript.clone())
            };

            {
                let mut transcription = transcription_state
                    .lock()
                    .expect("meeting engine lock poisoned");
                transcription.running = false;
                transcription.status = "completed".to_string();
                transcription.latency_ms = Some(done.latency_ms);
                transcription.text = Some(done.transcript);
                transcription.cleaned_text = cleaned_transcript;
                transcription.final_text = final_transcript_text;
                transcription.cleanup = cleanup;
                transcription.final_diarization = final_diarization;
                transcription.error = None;
            }
            // Checkpoint: transcript + diarization are on disk.
            if let Some(dir) = job_artifact_dir.as_deref() {
                write_meeting_state(dir, MEETING_PHASE_TRANSCRIBED, None);
                // The transcript is durable now — reclaim the disposable
                // intermediates (live/ windows + *.asr.wav copies), the bulk of
                // a meeting's footprint.
                prune_meeting_intermediates(dir);

                // Final stage: generate the meeting summary so the after-meeting
                // flow is complete and robust — NOT a separate manual click.
                // run_meeting_intelligence's LLM call already retries transient
                // failures; on a terminal failure we record it in meeting state
                // (phase stays "transcribed" + a "summary failed" error) so the UI
                // surfaces it and offers Retry, instead of silently stopping. On
                // success write_meeting_intelligence_cache marks the meeting
                // "summarized".
                if env_bool("AIRNOTE_MEETING_AUTO_SUMMARY", true) && !summary_text.trim().is_empty()
                {
                    if let Ok(mut t) = transcription_state.lock() {
                        t.status = "summarizing".to_string();
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
                    }
                }
            }
            JobOutcome::Done
        }
        Err(e) => {
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
                let mut transcription = transcription_state
                    .lock()
                    .expect("meeting engine lock poisoned");
                transcription.running = false;
                transcription.status = "failed".to_string();
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

#[tauri::command]
pub fn meeting_engine_start_session(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
    meeting_id: Option<String>,
) -> MeetingEngineStatus {
    tracing::info!(meeting_id = ?meeting_id, "[meeting_engine] start session");
    let status = state.start(meeting_id, Some(app.clone()));
    let _ = app.emit(STATUS_EVENT, status.clone());
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

#[tauri::command]
pub fn meeting_engine_stop_session(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> MeetingEngineStatus {
    tracing::info!("[meeting_engine] stop session");
    let status = state.stop();
    let _ = app.emit(STATUS_EVENT, status.clone());
    status
}

#[tauri::command]
pub fn meeting_engine_toggle_mute(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> MeetingEngineStatus {
    tracing::info!("[meeting_engine] toggle mute");
    let status = state.toggle_mute();
    let _ = app.emit(STATUS_EVENT, status.clone());
    status
}

#[tauri::command]
pub fn meeting_engine_get_status(
    app: AppHandle,
    state: State<'_, MeetingEngineState>,
) -> MeetingEngineStatus {
    state.emit_status(&app)
}

#[tauri::command]
pub fn meeting_engine_get_live_transcript(
    state: State<'_, MeetingEngineState>,
) -> MeetingLiveTranscriptPayload {
    state.live_transcript_payload()
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
pub fn meeting_engine_delete_meeting_files(meeting_id: String) -> Result<(), String> {
    let dir = meeting_dir_for_id(&meeting_id)?;
    if dir.is_dir() {
        fs::remove_dir_all(&dir).map_err(|e| format!("failed to delete meeting files: {e}"))?;
    }
    update_meeting_override(&meeting_id, |o| o.hidden = true)
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

/// Re-run transcription (whisper → cleanup → diarization) on a meeting's saved
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
    let _ = app.emit(STATUS_EVENT, status.clone());
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
                let _ = app.emit(
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

#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
fn start_system_capture(
    _path: PathBuf,
    _muted: Arc<AtomicBool>,
    _live_audio_tx: Option<mpsc::SyncSender<LiveAudioChunk>>,
) -> Result<SystemCaptureHandle, String> {
    Err("system audio capture is only available on macOS in this phase".to_string())
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
    let writer =
        create_audio_wav_writer(&path, "system").map_err(|e| report_ready_error(&ready_tx, e))?;
    let writer_path = path.clone();
    let writer_dropped_chunks = Arc::clone(&dropped_chunks);
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
    let _ = stream.stop_capture();
    drop(stream);
    drop(audio_tx);

    writer_join
        .join()
        .map_err(|_| "system audio writer thread panicked".to_string())?
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
) -> Result<MicCaptureSummary, String> {
    let mut samples_written = 0_u64;
    let mut peak_i16 = 0_i16;

    while let Ok(chunk) = audio_rx.recv() {
        for sample in chunk {
            writer
                .write_sample(sample)
                .map_err(|e| format!("failed to write {source_label} sample: {e}"))?;
            samples_written += 1;
            peak_i16 = peak_i16.max(sample.saturating_abs());
        }
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
        let mut live = snapshot.lock().expect("meeting engine lock poisoned");
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
    let join = thread::Builder::new()
        .name("meeting-live-transcript".to_string())
        .spawn(move || {
            run_live_transcript_worker(session, config, live_dir, snapshot, app, audio_rx, stop_rx);
            let _ = done_tx.send(());
        })
        .map_err(|e| format!("failed to spawn live transcript worker: {e}"))?;

    Ok(LiveTranscriptHandle {
        audio_tx,
        stop_tx,
        done_rx,
        join: Some(join),
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

        if stop_rx.try_recv().is_ok() {
            stop_requested = true;
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
            false,
        );
    }

    while let Ok(chunk) = audio_rx.try_recv() {
        push_live_audio_chunk(&mut mic, &mut system, chunk);
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
        true,
    );

    let mut live = snapshot.lock().expect("meeting engine lock poisoned");
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
    force: bool,
) {
    for track in [mic, system] {
        loop {
            let Some(window) =
                track.take_ready_window(config.chunk_samples, config.min_samples, force)
            else {
                break;
            };
            match transcribe_live_window(session, config, live_dir, window, *chunk_index) {
                Ok(chunks) => {
                    for chunk in chunks {
                        *chunk_index = chunk_index.saturating_add(1);
                        append_live_transcript_chunk(snapshot, app, &session.session_id, chunk);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[meeting_engine] live transcript window failed");
                    let mut live = snapshot.lock().expect("meeting engine lock poisoned");
                    live.error = Some(e);
                    if live.status == "running" {
                        live.status = "running_with_errors".to_string();
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
    app: Option<&AppHandle>,
    session_id: &str,
    chunk: MeetingLiveTranscriptChunk,
) {
    {
        let mut live = snapshot.lock().expect("meeting engine lock poisoned");
        live.chunks.push(chunk.clone());
        live.status = "running".to_string();
        live.error = None;
    }
    if let Some(app) = app {
        let _ = app.emit(
            LIVE_TRANSCRIPT_EVENT,
            MeetingLiveTranscriptEvent {
                session_id: session_id.to_string(),
                chunk,
            },
        );
    }
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
    )?;
    let segments = label_transcript_segments(
        &done,
        source.source_label(),
        source.speaker_id(),
        source.speaker_name(),
        summary.duration_ms,
    );

    let mut chunks = Vec::new();
    for (offset, segment) in segments.into_iter().enumerate() {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        chunks.push(MeetingLiveTranscriptChunk {
            chunk_index: next_chunk_index.saturating_add(offset as u64),
            source: segment.source,
            speaker_id: segment.speaker_id,
            speaker_name: segment.speaker_name,
            timestamp_ms: start_ms.saturating_add(segment.start_ms),
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
/// (transcribing → cleaning → diarizing → done) and a Retry affordance. Read
/// from disk (so it survives restarts and works for background jobs) and overlaid
/// with the live queue/worker state.
#[derive(Debug, Serialize)]
pub struct MeetingProcessingStatusPayload {
    pub meeting_id: String,
    pub phase: String,
    pub stage: String,
    pub running: bool,
    pub queued: bool,
    pub can_retry: bool,
    pub error: Option<String>,
    pub has_transcript: bool,
    pub has_intelligence: bool,
    /// Transcript is done but the summary stage failed (recoverable via
    /// regenerate, distinct from a transcription failure that needs re-transcribe).
    pub summary_failed: bool,
    pub updated_at_ms: u64,
}

#[tauri::command]
pub fn meeting_engine_get_processing_status(
    state: State<'_, MeetingEngineState>,
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
    let in_flight = {
        let inner = state.jobs.lock();
        inner.in_flight.as_deref() == Some(meeting_id.as_str())
    };
    let queued = !in_flight && state.jobs.is_active(&meeting_id);
    let running = in_flight || queued;

    // Fine-grained stage. The global transcription snapshot only describes the
    // in-flight meeting, so we only trust it when THIS meeting is in flight.
    let stage = if in_flight {
        let snapshot = state
            .transcription
            .lock()
            .expect("meeting engine lock poisoned");
        match snapshot.status.as_str() {
            "running" | "" => "transcribing",
            "cleaning" => "cleaning",
            "completed" => "diarizing",
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

    if running {
        error = None;
    }

    let terminal_phase = matches!(
        phase.as_str(),
        MEETING_PHASE_TRANSCRIBED | MEETING_PHASE_SUMMARIZED | MEETING_PHASE_FAILED
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
        && (phase == MEETING_PHASE_FAILED || (!terminal_phase && !has_transcript));

    Ok(MeetingProcessingStatusPayload {
        meeting_id,
        phase,
        stage,
        running,
        queued,
        can_retry,
        error,
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
fn cleanup_empty_session_dir(dir: &Path) {
    let has_audio = RECOVERABLE_MEETING_WAVS
        .iter()
        .any(|name| dir.join(name).is_file());
    if has_audio || meeting_has_usable_transcript(dir) {
        return;
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), "[meeting_engine] removed empty meeting dir (no audio captured)")
        }
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "[meeting_engine] failed to remove empty meeting dir")
        }
    }
}

/// Delete the disposable intermediates once a meeting has a final transcript:
/// the per-window `live/` WAVs (+ their whisper sidecars) and the `*.asr.wav`
/// gain-normalized copies. These are only needed during transcription and are
/// the bulk of a meeting's disk footprint (hundreds of files / hundreds of MB on
/// a long meeting). The source `mic.wav`/`system.wav` and the transcript/summary
/// artifacts are kept.
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
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".asr.wav"))
            {
                let _ = fs::remove_file(&path);
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
///     intermediates (live/ windows + *.asr.wav). This also reclaims the
///     intermediates left by meetings that completed before pruning existed.
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

        let is_local_placeholder = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("local-"));
        if is_local_placeholder && !has_audio && !has_transcript {
            if fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
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
            e.file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".asr.wav"))
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
            Some(MEETING_PHASE_TRANSCRIBED | MEETING_PHASE_SUMMARIZED | MEETING_PHASE_FAILED) => {
                continue;
            }
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

fn whisper_language_for_track(track: MeetingAudioTrack, default_language: &str) -> String {
    match track {
        MeetingAudioTrack::Mic => meeting_env("AIRNOTE_MEETING_MIC_WHISPER_LANGUAGE")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_language.to_string()),
        // The system track carries the remote participant — usually the most
        // important voice. Default it to the SAME resolved meeting language as
        // the mic, not "auto": auto-detect hallucinates random scripts on quiet
        // segments / speaker bleed (same reason the mic defaults to a fixed
        // language). Set AIRNOTE_MEETING_SYSTEM_WHISPER_LANGUAGE=auto to opt back
        // into detection.
        MeetingAudioTrack::System => meeting_env("AIRNOTE_MEETING_SYSTEM_WHISPER_LANGUAGE")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_language.to_string()),
    }
}

fn whisper_translate_for_track(track: MeetingAudioTrack) -> bool {
    match track {
        MeetingAudioTrack::Mic => env_bool("AIRNOTE_MEETING_MIC_WHISPER_TRANSLATE", false),
        MeetingAudioTrack::System => env_bool("AIRNOTE_MEETING_SYSTEM_WHISPER_TRANSLATE", false),
    }
}

fn transcribe_meeting_plan(
    plan: &MeetingTranscriptionPlan,
    config: &WhisperCppConfig,
) -> Result<MeetingPlanTranscriptionDone, String> {
    let started = Instant::now();
    let mic_paths = transcript_paths_for_wav(&plan.mic.path);

    let Some(system_summary) = &plan.system else {
        if !has_transcribable_audio(&plan.mic) {
            return Err(format!(
                "mic track peak {:.6} is below speech threshold; skipping transcription",
                plan.mic.peak
            ));
        }
        let mic_done =
            transcribe_with_whisper_cpp(&plan.mic, &mic_paths, config, MeetingAudioTrack::Mic)?;
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
        Some(transcribe_with_whisper_cpp(
            &plan.mic,
            &mic_paths,
            config,
            MeetingAudioTrack::Mic,
        ))
    } else {
        tracing::warn!(
            peak = plan.mic.peak,
            samples_written = plan.mic.samples_written,
            "[meeting_engine] mic audio below speech threshold; skipping mic ASR"
        );
        None
    };
    let system_paths = transcript_paths_for_wav(&system_summary.path);
    let system_result = if has_transcribable_audio(system_summary) {
        Some(transcribe_with_whisper_cpp(
            system_summary,
            &system_paths,
            config,
            MeetingAudioTrack::System,
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
) -> Result<WhisperTranscriptionDone, String> {
    transcribe_with_whisper_cpp_for(summary, paths, config, track, WHISPER_TIMEOUT)
}

fn transcribe_with_whisper_cpp_for(
    summary: &MicCaptureSummary,
    paths: &TranscriptPaths,
    config: &WhisperCppConfig,
    track: MeetingAudioTrack,
    timeout: Duration,
) -> Result<WhisperTranscriptionDone, String> {
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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

    let started = Instant::now();
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn whisper.cpp: {e}"))?;
    let output = wait_with_timeout(child, timeout)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "whisper.cpp exited with {}: {}",
            output.status,
            truncate_error(if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            })
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

fn wait_with_timeout(child: std::process::Child, timeout: Duration) -> Result<Output, String> {
    wait_with_timeout_for(child, timeout, "whisper.cpp")
}

fn wait_with_timeout_for(
    mut child: std::process::Child,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("failed to collect {label} output: {e}"));
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{label} timed out after {}s", timeout.as_secs()));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("failed to poll {label}: {e}")),
        }
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

    let config = match meeting_final_diarization_config() {
        Ok(Some(config)) => config,
        Ok(None) => {
            return MeetingFinalDiarizationSnapshot::skipped(
                "skipped_missing_command",
                "AIRNOTE_MEETING_FINAL_DIARIZATION_COMMAND or AIRNOTE_MEETING_FINAL_DIARIZATION_SCRIPT is not set",
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

    {
        let mut transcription = transcription_state
            .lock()
            .expect("meeting engine lock poisoned");
        transcription.status = "final_diarizing".to_string();
        transcription.final_diarization =
            MeetingFinalDiarizationSnapshot::running(config.provider.clone(), &paths);
    }

    let started = Instant::now();
    let result = run_final_diarization_command(&config, audio_path, transcript_paths, &paths);
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(()) => MeetingFinalDiarizationSnapshot::completed(config.provider, latency_ms, &paths),
        Err(e) => {
            write_final_diarization_failure(&paths.diarization_json, &config.provider, &e);
            MeetingFinalDiarizationSnapshot::failed(config.provider, latency_ms, &paths, e)
        }
    }
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
    let diarization_json_path =
        diarization_path_for_transcript(paths).map(|path| path.to_string_lossy().to_string());
    let final_paths = final_diarization_paths_for_transcript(paths);
    let final_diarization_json_path = final_paths
        .as_ref()
        .map(|paths| paths.diarization_json.to_string_lossy().to_string());
    let final_transcript_json_path = final_paths
        .as_ref()
        .map(|paths| paths.transcript_json.to_string_lossy().to_string());
    let artifact = MeetingTranscriptArtifact {
        schema_version: 1,
        provider: "whisper.cpp".to_string(),
        status: status.to_string(),
        language: Some(language.to_string()),
        model: config.map(|config| config.model.to_string_lossy().to_string()),
        source_wav: summary.path.to_string_lossy().to_string(),
        source_wavs,
        diarization_json_path: diarization_json_path.clone(),
        final_diarization_json_path,
        final_transcript_json_path,
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

    if let Some(path) = diarization_path_for_transcript(paths) {
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
                tracing::warn!(error = %e, path = %path.display(), "[meeting_engine] failed to write diarization json");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "[meeting_engine] failed to serialize diarization json");
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
        let _ = fs::create_dir_all(&dir);
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

    if let Some(session) = state
        .session
        .lock()
        .expect("meeting engine lock poisoned")
        .clone()
    {
        dirs.push(session.artifact_dir);
    }

    let transcription = state
        .transcription
        .lock()
        .expect("meeting engine lock poisoned")
        .clone();
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

#[derive(Debug, Deserialize)]
struct CleanupChatResponse {
    choices: Vec<CleanupChatChoice>,
}

#[derive(Debug, Deserialize)]
struct CleanupChatChoice {
    message: CleanupChatMessage,
}

#[derive(Debug, Deserialize)]
struct CleanupChatMessage {
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
                    thread::sleep(Duration::from_millis(800 * attempt as u64));
                    continue;
                }
                return Err(format!("meeting AI request failed: {e}"));
            }
        };
        let status = response.status();
        if status.is_success() {
            break response;
        }
        let retryable = status.as_u16() == 429 || status.is_server_error();
        let body_text = response.text().unwrap_or_default();
        if retryable && attempt < MEETING_LLM_MAX_ATTEMPTS {
            tracing::warn!(attempt, %status, "[meeting_engine] LLM transient error; retrying");
            thread::sleep(Duration::from_millis(800 * attempt as u64));
            continue;
        }
        return Err(format!(
            "meeting AI provider error {status}: {}",
            truncate_error(body_text.trim())
        ));
    };

    let response: CleanupChatResponse = response
        .json()
        .map_err(|e| format!("meeting AI response parse failed: {e}"))?;
    let content = response
        .choices
        .first()
        .map(|choice| choice.message.content.as_str())
        .unwrap_or("");
    if content.trim().is_empty() {
        return Err("meeting AI returned an empty response".to_string());
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
    let response = client
        .post(&config.url)
        .header(&config.auth_header_name, &config.auth_header_value)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("meeting AI request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().unwrap_or_default();
        return Err(format!(
            "meeting AI provider error {status}: {}",
            truncate_error(body_text.trim())
        ));
    }

    let mut reader = std::io::BufReader::new(response);
    let mut line = String::new();
    let mut content = String::new();
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
        return Err("meeting AI returned an empty response".to_string());
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

    let ai_provider = env_nonempty("AIRNOTE_MEETING_AI_PROVIDER");
    let provider = ai_provider
        .clone()
        .unwrap_or_else(meeting_cleanup_provider)
        .to_ascii_lowercase();
    let model = env_nonempty("AIRNOTE_MEETING_AI_MODEL").unwrap_or_else(|| {
        if ai_provider.is_some() {
            default_meeting_model(&provider)
        } else {
            meeting_cleanup_model(&provider)
        }
    });
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
                    "GATEWAY_API_KEY not set; meeting AI not run with Gateway".to_string()
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
            let api_key = override_key
                .or_else(|| env_nonempty("DEEPSEEK_API_KEY"))
                .ok_or_else(|| {
                    "DEEPSEEK_API_KEY not set; meeting AI not run with DeepSeek".to_string()
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

fn meeting_ai_verification_enabled() -> bool {
    env_bool("AIRNOTE_MEETING_AI_VERIFY", true)
}

fn meeting_cleanup_provider() -> String {
    env_nonempty("AIRNOTE_MEETING_CLEANUP_PROVIDER")
        .unwrap_or_else(|| DEFAULT_MEETING_CLEANUP_PROVIDER.to_string())
        .to_ascii_lowercase()
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
    env_bool("AIRNOTE_MEETING_FINAL_DIARIZATION_ENABLED", false)
}

fn meeting_final_diarization_config() -> Result<Option<MeetingFinalDiarizationConfig>, String> {
    said_core::load_env();

    let timeout = Duration::from_secs(env_u64(
        "AIRNOTE_MEETING_FINAL_DIARIZATION_TIMEOUT_SECS",
        DEFAULT_FINAL_DIARIZATION_TIMEOUT_SECS,
    ));
    let provider = env_nonempty("AIRNOTE_MEETING_FINAL_DIARIZATION_PROVIDER")
        .unwrap_or_else(|| "nemo_sortformer".to_string());

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

/// List installed whisper.cpp models (`ggml-*.bin`, excluding the Silero VAD
/// model) in the app's model directory, marking which one is currently active.
#[tauri::command]
pub fn meeting_list_whisper_models() -> Vec<WhisperModelInfo> {
    let dir = said_core::paths::data_dir().join("models");
    let active = resolve_whisper_cpp_config().ok().map(|config| config.model);
    let mut models = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("ggml-") || !name.ends_with(".bin") || name.contains("silero") {
                continue;
            }
            // fs::metadata follows symlinks (some models are symlinked to another
            // dir), so the size is the real target size, not the link's ~90 bytes.
            let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            models.push(WhisperModelInfo {
                name: name.to_string(),
                path: path.display().to_string(),
                size_bytes,
                active: active.as_deref() == Some(path.as_path()),
                incomplete: size_bytes < MIN_WHISPER_MODEL_BYTES,
            });
        }
    }
    models.sort_by(|a, b| a.name.cmp(&b.name));
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
/// estimate before Content-Length is known). Mirror of the official
/// ggerganov/whisper.cpp ggml weights.
const WHISPER_MODEL_CATALOG: &[(&str, &str, u64)] = &[
    (
        "ggml-base.en.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        147_951_465,
    ),
    (
        "ggml-small.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        487_601_967,
    ),
    (
        "ggml-medium.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        1_533_763_059,
    ),
    (
        "ggml-large-v3-turbo.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        1_624_555_275,
    ),
    (
        "ggml-large-v3.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        3_095_033_483,
    ),
];

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

#[tauri::command]
pub fn meeting_whisper_model_catalog() -> Vec<CatalogModel> {
    let dir = said_core::paths::data_dir().join("models");
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
    let dir = said_core::paths::data_dir().join("models");
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
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_whisper_model_blocking(&app, &name_for_task, &url, total_hint, &dir, &dest)
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?;

    if let Ok(mut inflight) = model_downloads_inflight().lock() {
        inflight.remove(&name);
    }
    if let Ok(mut cancels) = model_download_cancels().lock() {
        cancels.remove(&name);
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
        let _ = app.emit(
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
    // Sanity: a truncated download (server hiccup) shouldn't masquerade as a model.
    if total > 0 && received < total / 2 {
        return Err(fail(
            &part,
            received,
            total,
            "download ended early (incomplete file)".to_string(),
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
    let path = said_core::paths::data_dir().join("models").join(&name);
    if !path.is_file() {
        return Err("model is not installed".to_string());
    }
    fs::remove_file(&path).map_err(|e| format!("couldn't delete model: {e}"))?;
    if meeting_env("AIRNOTE_WHISPER_CPP_MODEL").as_deref() == Some(&path.display().to_string()) {
        let _ = meeting_settings_set("AIRNOTE_WHISPER_CPP_MODEL".to_string(), None);
    }
    // Re-point the active selection to a remaining model (or clear it).
    meeting_ensure_active_model();
    Ok(())
}

/// Guarantee an active model whenever any usable one exists. Keeps the current
/// selection if it's still a real file; otherwise auto-selects the strongest
/// installed model and persists it (so a single installed model is always
/// active). Clears the setting if no usable model remains. Returns the active
/// model's file name, or None when none is installed.
#[tauri::command]
pub fn meeting_ensure_active_model() -> Option<String> {
    let name_of = |path: &Path| -> Option<String> {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
    };
    // Keep the current selection if it still points at a real, usable model.
    if let Some(current) = env_path("AIRNOTE_WHISPER_CPP_MODEL") {
        if is_usable_whisper_model(&current) {
            return name_of(&current);
        }
    }
    // Otherwise auto-select the strongest installed model and persist it.
    let dir = said_core::paths::data_dir().join("models");
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
        // Bundled sidecar inside the shipped .app (build-dmg.sh copies whisper-cli
        // into Contents/MacOS next to the app binary). Preferred over PATH so a
        // packaged install transcribes without any dev tooling present.
        .or_else(find_bundled_whisper_cli)
        .or_else(|| find_on_path("whisper-cli"))
        .or_else(|| find_on_path("main"))
        .ok_or_else(|| {
            "whisper.cpp binary not found; set AIRNOTE_WHISPER_CPP_BIN or WHISPER_CPP_BIN"
                .to_string()
        })?;

    let model = env_path("AIRNOTE_WHISPER_CPP_MODEL")
        .or_else(|| env_path("WHISPER_CPP_MODEL"))
        .or_else(default_whisper_model_path)
        .ok_or_else(|| {
            "whisper.cpp model not found; set AIRNOTE_WHISPER_CPP_MODEL or WHISPER_CPP_MODEL"
                .to_string()
        })?;

    let language = meeting_env("AIRNOTE_MEETING_WHISPER_LANGUAGE")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WHISPER_LANGUAGE.to_string());
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
        env_path("AIRNOTE_MEETING_VAD_MODEL")
            .or_else(|| model.parent().and_then(find_silero_vad_model))
            // The whisper model may resolve to the dev repo (tools/stt-bench),
            // whose folder has no Silero model, so also check the app's data
            // models dir where the VAD model is downloaded.
            .or_else(|| find_silero_vad_model(&said_core::paths::data_dir().join("models")))
            // Bundled with the shipped .app (Contents/MacOS or Contents/Resources/models).
            .or_else(|| {
                bundled_models_dirs()
                    .iter()
                    .find_map(|d| find_silero_vad_model(d))
            })
    } else {
        None
    };
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

/// Locate a `whisper-cli` binary bundled inside the shipped app. The DMG build
/// copies it into `Contents/MacOS` next to the app executable; the dev/release
/// target dirs are also walked so `just dev` finds a synced copy. None if absent
/// (callers then fall back to PATH).
fn find_bundled_whisper_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        if let Some(d) = dir {
            candidates.push(d.join("whisper-cli"));
            candidates.push(d.join("debug").join("whisper-cli"));
            candidates.push(d.join("release").join("whisper-cli"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    candidates.into_iter().find(|p| p.is_file())
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
            dirs.push(d.join("..").join("Resources").join("models"));
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
            if best.as_ref().map(|b| path > *b).unwrap_or(true) {
                best = Some(path);
            }
        }
    }
    best
}

fn resolve_live_transcript_config() -> Result<LiveTranscriptConfig, String> {
    let mut whisper = resolve_whisper_cpp_config()
        .map_err(|e| format!("live transcript requires whisper.cpp; {e}"))?;
    if let Some(model) = live_whisper_model_path() {
        whisper.model = model;
    }
    if let Some(language) = env_nonempty("AIRNOTE_MEETING_LIVE_WHISPER_LANGUAGE") {
        whisper.language = language;
    }
    let chunk_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_CHUNK_SECS",
        DEFAULT_LIVE_TRANSCRIPT_CHUNK_SECS,
    )
    .clamp(3, 60);
    let min_secs = env_u64(
        "AIRNOTE_MEETING_LIVE_TRANSCRIPT_MIN_SECS",
        DEFAULT_LIVE_TRANSCRIPT_MIN_SECS,
    )
    .clamp(1, chunk_secs);
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
        chunk_samples: chunk_secs.saturating_mul(SAMPLE_RATE as u64) as usize,
        min_samples: min_secs.saturating_mul(SAMPLE_RATE as u64) as usize,
        poll_interval: Duration::from_millis(poll_ms),
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn live_whisper_model_path() -> Option<PathBuf> {
    env_path("AIRNOTE_MEETING_LIVE_WHISPER_CPP_MODEL")
        .or_else(|| env_path("WHISPER_CPP_LIVE_MODEL"))
        .or_else(|| {
            env_dir("AIRNOTE_MEETING_LIVE_WHISPER_CPP_MODEL_DIR")
                .or_else(|| env_dir("WHISPER_CPP_LIVE_MODEL_DIR"))
                .and_then(|dir| first_live_whisper_model_in_dir(&dir))
        })
        .or_else(default_dev_repo_live_whisper_model_path)
}

fn default_dev_repo_live_whisper_model_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(option_env!("CARGO_MANIFEST_DIR")?);
    let repo_dir = manifest_dir.parent()?.parent()?;
    first_live_whisper_model_in_dir(&repo_dir.join("tools").join("stt-bench").join("models"))
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

fn first_live_whisper_model_in_dir(model_dir: &Path) -> Option<PathBuf> {
    [
        "ggml-large-v3-turbo.bin",
        "ggml-small.bin",
        "ggml-small.en.bin",
        "ggml-medium.bin",
        "ggml-medium.en.bin",
        "ggml-large-v3.bin",
    ]
    .into_iter()
    .map(|name| model_dir.join(name))
    .find(|path| is_usable_whisper_model(path))
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
    vec![
        "ggml-large-v3-turbo.bin",
        "ggml-large-v3.bin",
        "ggml-large-v2.bin",
        "ggml-medium.bin",
        "ggml-medium.en.bin",
        "ggml-small.bin",
        "ggml-small.en.bin",
        "ggml-base.bin",
        "ggml-base.en.bin",
        "ggml-tiny.bin",
        "ggml-tiny.en.bin",
    ]
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
        fs::write(dir.join("live").join("live-mic-0.wav"), b"x").unwrap();
        fs::write(dir.join("mic.asr.wav"), b"x").unwrap();
        fs::write(dir.join("system.asr.wav"), b"x").unwrap();
        fs::write(dir.join("mic.wav"), b"x").unwrap();
        fs::write(dir.join("meeting.transcript.json"), b"{}").unwrap();
        prune_meeting_intermediates(&dir);
        assert!(!dir.join("live").exists(), "live/ windows pruned");
        assert!(!dir.join("mic.asr.wav").exists(), ".asr.wav pruned");
        assert!(!dir.join("system.asr.wav").exists());
        assert!(dir.join("mic.wav").exists(), "source WAV kept");
        assert!(
            dir.join("meeting.transcript.json").exists(),
            "transcript kept"
        );
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
    fn transcript_artifact_writes_diarization_json_with_person_labels() {
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
        let final_diarization_path = dir.join("meeting.diarization.final.json");
        let final_transcript_path = dir.join("meeting.transcript.final.json");
        let diarization_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&diarization_path).unwrap()).unwrap();

        assert_eq!(
            transcript_json["diarization_json_path"].as_str().unwrap(),
            diarization_path.to_string_lossy()
        );
        assert_eq!(
            transcript_json["final_diarization_json_path"]
                .as_str()
                .unwrap(),
            final_diarization_path.to_string_lossy()
        );
        assert_eq!(
            transcript_json["final_transcript_json_path"]
                .as_str()
                .unwrap(),
            final_transcript_path.to_string_lossy()
        );
        assert_eq!(transcript_json["segments"][0]["source"], "mic");
        assert_eq!(transcript_json["segments"][0]["speaker_name"], "You");
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
        let audio = state
            .audio
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();

        assert_eq!(plan.mic.path, mic_path);
        assert_eq!(
            plan.system.as_ref().map(|summary| &summary.path),
            Some(&system_path)
        );
        assert!(plan.summary.path.ends_with("meeting.merged.wav"));
        assert!(plan.output_paths.text.ends_with("meeting.transcript.txt"));
        assert_eq!(plan.source_wavs, vec![mic_path, system_path]);
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
        let audio = state
            .audio
            .lock()
            .expect("meeting engine lock poisoned")
            .clone();

        assert_eq!(plan.system.as_ref().map(|summary| &summary.path), None);
        assert!(plan.output_paths.text.ends_with("meeting.transcript.txt"));
        assert_eq!(plan.source_wavs, vec![mic_path]);
        assert_eq!(audio.status, "skipped_silent_system_audio");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn meeting_whisper_defaults_prefer_turbo_for_optimized_meeting_flow() {
        let candidates = default_whisper_model_candidates(Path::new("/tmp/airnote-test-data"));

        assert!(candidates[0].ends_with("models/ggml-large-v3-turbo.bin"));
        assert!(
            candidates
                .iter()
                .position(|path| path.ends_with("models/ggml-large-v3.bin"))
                < candidates
                    .iter()
                    .position(|path| path.ends_with("models/ggml-small.bin"))
        );
    }

    #[test]
    fn meeting_whisper_model_dir_chooses_strongest_installed_model() {
        let dir = std::env::temp_dir().join(format!(
            "airnote-whisper-model-dir-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        // Real-sized stubs: the resolver skips empty/partial models (< 1 MB).
        let blob = vec![0u8; (MIN_WHISPER_MODEL_BYTES + 1) as usize];
        fs::write(dir.join("ggml-small.bin"), &blob).unwrap();
        fs::write(dir.join("ggml-large-v3-turbo.bin"), &blob).unwrap();

        let selected = first_whisper_model_in_dir(&dir).unwrap();

        assert!(selected.ends_with("ggml-large-v3-turbo.bin"));

        // A 0-byte placeholder must be ignored, not chosen.
        let empty_dir = dir.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        fs::write(empty_dir.join("ggml-large-v3-turbo.bin"), b"").unwrap();
        assert!(first_whisper_model_in_dir(&empty_dir).is_none());

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
