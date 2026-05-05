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
}

export interface AppSnapshot {
  state: AppState;
  platform: string;
  current_mode: string;
  current_mode_label: string;
  current_model: string;
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

// ── Backend types (mirrored from polish-backend) ─────────────────────────────

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
  // API keys stored in SQLite — never leave the device
  gateway_api_key:    string | null;
  deepgram_api_key:   string | null;
  gemini_api_key:     string | null;
  groq_api_key:       string | null;
  /** LLM routing: "gateway" (default) | "gemini_direct" | "groq" | "openai_codex" */
  llm_provider:       string;
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
  // API keys — set to null to clear
  gateway_api_key?:    string | null;
  deepgram_api_key?:   string | null;
  gemini_api_key?:     string | null;
  groq_api_key?:       string | null;
  /** LLM routing: "gateway" | "gemini_direct" | "groq" | "openai_codex" */
  llm_provider?:       string;
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
}

/** Backend endpoint info (url + shared secret) */
export interface BackendEndpoint {
  url:    string;
  secret: string;
}

/** Streaming result from a polish operation */
export interface PolishDone {
  recording_id:  string;
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

// ── OpenAI OAuth status ──────────────────────────────────────────────────────

export interface OpenAIStatus {
  connected:    boolean;
  expires_at?:  number;      // unix ms
  connected_at?: number;     // unix ms
  model_smart:  string;      // "gpt-5.4"
  model_mini:   string;      // "gpt-5.4-mini"
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

// ── Display helpers ──────────────────────────────────────────────────────────

export interface TimelineItem {
  time: string;
  text: string;
  word_count?: number;
  model?: string;
  timestamp_ms: number;
}

export interface TimelineGroup {
  label: string;
  items: TimelineItem[];
}

/** Group HistoryItem[] (newest-first) into display groups by calendar day */
export function groupHistory(history: HistoryItem[]): TimelineGroup[] {
  if (history.length === 0) return [];

  const now = Date.now();
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const todayMs = startOfToday.getTime();
  const yesterdayMs = todayMs - 86_400_000;

  const buckets = new Map<string, TimelineItem[]>();

  for (const item of history) {
    const d = new Date(item.timestamp_ms);

    const startOfItemDay = new Date(item.timestamp_ms);
    startOfItemDay.setHours(0, 0, 0, 0);
    const itemDayMs = startOfItemDay.getTime();

    let label: string;
    if (itemDayMs >= todayMs) {
      label = "TODAY";
    } else if (itemDayMs >= yesterdayMs) {
      label = "YESTERDAY";
    } else {
      label = d
        .toLocaleDateString("en-US", {
          month: "long",
          day: "numeric",
          year: "numeric",
        })
        .toUpperCase();
    }

    const time = d.toLocaleTimeString("en-US", {
      hour: "2-digit",
      minute: "2-digit",
    });

    const existing = buckets.get(label) ?? [];
    existing.push({
      time,
      text: item.polished,
      word_count: item.word_count,
      model: item.model,
      timestamp_ms: item.timestamp_ms,
    });
    buckets.set(label, existing);
  }

  return Array.from(buckets.entries()).map(([label, items]) => ({
    label,
    items,
  }));
}
