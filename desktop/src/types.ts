export type AppState = "idle" | "recording" | "processing";

export interface Mode {
  key: string;
  label: string;
  model: string;
  icon: string;
}

export interface LastResult {
  transcript: string;
  polished: string;
  model: string;
  confidence: number;
  transcribe_ms: number;
  polish_ms: number;
}

/** A single persisted recording entry */
export interface HistoryItem {
  timestamp_ms: number;
  polished: string;
  word_count: number;
  recording_seconds: number;
  model: string;
  transcribe_ms: number;
  embed_ms: number;
  polish_ms: number;
  audio_id: string | null;
  edit_count: number;
}

export interface AppSnapshot {
  state: AppState;
  recording_id?: string | null;
  platform: string;
  current_mode: string;
  current_mode_label: string;
  current_model: string;
  message_polish_mode?: boolean;
  auto_paste_supported:     boolean;
  accessibility_granted:    boolean;
  microphone_granted:       boolean;
  input_monitoring_granted: boolean;
  screen_recording_granted: boolean;
  modes: Mode[];
  last_result: LastResult | null;
  last_error: string | null;
  /** Last 100 recordings, newest first */
  history: HistoryItem[];
  total_words: number;
  daily_streak: number;
  /** Rolling average WPM over last 10 recordings */
  avg_wpm: number;
}

// ── Backend types (mirrored from airnote-backend) ────────────────────────────

/** STT provider from preferences plus key readiness (for API key section). */
export interface SttRuntimeInfo {
  provider: string;
  preferred_provider: string;
  effective_provider: string;
  deepgram_configured: boolean;
  swift_installed: boolean;
  swift_ready: boolean;
  whisper_installed: boolean;
  whisper_ready: boolean;
  whisper_vad_installed: boolean;
}

export interface Preferences {
  user_id:            string;
  selected_model:     string;
  tone_preset:        string;
  custom_prompt:      string | null;
  language:           string;
  output_language:    string;   // "hinglish" | "hindi" | "english"
  auto_paste:         boolean;
  edit_capture:       boolean;
  polish_text_hotkey: string;
  record_hotkey:      string;
  learning_enabled:   boolean;
  server_runtime_enabled: boolean;
  server_audio_runtime_enabled: boolean;
  // API keys stored in SQLite — never leave the device
  gateway_api_key:    string | null;
  deepgram_api_key:   string | null;
  gemini_api_key:     string | null;
  groq_api_key:       string | null;
  cerebras_api_key:   string | null;
  deepinfra_api_key:  string | null;
  /** LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex" */
  llm_provider:       string;
  /** STT routing: "deepgram" | "swift_local" | "whisper_local" */
  stt_provider:       string;
}

export interface PrefsUpdate {
  selected_model?:     string;
  tone_preset?:        string;
  custom_prompt?:      string | null;
  language?:           string;
  output_language?:    string;
  auto_paste?:         boolean;
  edit_capture?:       boolean;
  polish_text_hotkey?: string;
  record_hotkey?:      string;
  learning_enabled?:   boolean;
  server_runtime_enabled?: boolean;
  server_audio_runtime_enabled?: boolean;
  // API keys — set to null to clear
  gateway_api_key?:    string | null;
  deepgram_api_key?:   string | null;
  gemini_api_key?:     string | null;
  groq_api_key?:       string | null;
  cerebras_api_key?:   string | null;
  deepinfra_api_key?:  string | null;
  /** LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex" */
  llm_provider?:       string;
  /** STT routing: "deepgram" | "swift_local" | "whisper_local" */
  stt_provider?:       string;
}

export interface PolishModelEntry {
  key: string;
  label: string;
  provider: string;
  model_id: string;
  beta_only: boolean;
  available: boolean;
}

export interface ListPolishModelsResponse {
  models: PolishModelEntry[];
  selected_model: string;
}

export interface ProcessPerf {
  pid: number;
  name: string;
  cpu_percent: number;
  memory_bytes: number;
  virtual_memory_bytes: number;
}

export interface GpuPerf {
  available: boolean;
  label: string;
  utilization_percent: number | null;
  memory_bytes: number | null;
}

export interface PerformanceSnapshot {
  timestamp_ms: number;
  cpu_percent: number;
  physical_core_count: number | null;
  total_memory_bytes: number;
  used_memory_bytes: number;
  available_memory_bytes: number;
  total_swap_bytes: number;
  used_swap_bytes: number;
  desktop: ProcessPerf | null;
  backend: ProcessPerf | null;
  gpu: GpuPerf;
}

/** Full recording row from backend SQLite */
export interface Recording {
  id:                string;
  timestamp_ms:      number;
  transcript:        string;
  polished:          string;
  final_text:        string | null;
  word_count:        number;
  recording_seconds: number;
  model_used:        string;
  confidence:        number | null;
  transcribe_ms:     number | null;
  embed_ms:          number | null;
  polish_ms:         number | null;
  target_app:        string | null;
  edit_count:        number;
  source:            string;
  audio_id:          string | null;
  enriched_transcript: string | null;
  raw_transcript: string | null;
  local_corrected_transcript: string | null;
  polished_output: string | null;
}

/** Resolved identity of an app the user dictated into (name + category + icon). */
export interface AppIdentity {
  key:      string;
  name:     string | null;
  category: string | null;
  icon:     string | null; // data:image/png;base64,…
}

/** Per-app dictation usage row (Insights "apps you dictate in"). */
export interface AppUsageRow {
  app:          string; // the target_app key (bundle-id / exe path)
  count:        number;
  total_words:  number;
  last_used_ms: number;
}

/** Per-site dictation usage row (Insights "sites you dictate in"). Host only. */
export interface SiteUsageRow {
  host:         string; // domain only, e.g. mail.google.com
  target_app:   string; // the browser bundle-id it was seen in
  count:        number;
  last_used_ms: number;
}

/** Backend endpoint info (url + shared secret) */
export interface BackendEndpoint {
  url:    string;
  secret: string;
}

/** Streaming result from a polish operation */
export interface PolishDone {
  recording_id:  string;
  run_id?:       string | null;
  transcript:    string;
  polished:      string;
  model_used:    string;
  confidence:    number | null;
  audio_id?:     string | null;
  source?:       string | null;
  target_app?:   string | null;
  output_language?: string | null;
  enriched_transcript?: string | null;
  examples_used: number;
  latency_ms: {
    transcribe: number;
    embed:      number;
    retrieve:   number;
    polish:     number;
    total:      number;
  };
}

// ── Cloud auth types ─────────────────────────────────────────────────────────

export interface CloudAccount {
  id:           string;
  email:        string;
  license_tier: string;
}

export interface CloudAuthResponse {
  token:   string;
  account: CloudAccount;
}

export interface CloudStatus {
  connected:    boolean;
  license_tier: string;
  email:        string | null;
}

// ── Pending edits ────────────────────────────────────────────────────────────

export interface PendingEdit {
  id:           string;
  recording_id: string | null;
  ai_output:    string;
  user_kept:    string;
  timestamp_ms: number;
}

export interface PendingEditsResponse {
  edits: PendingEdit[];
  total: number;
}
