import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppIdentity,
  AppUsageRow,
  SiteUsageRow,
  AppSnapshot,
  BackendEndpoint,
  CloudAuthResponse,
  CloudStatus,
  HistoryItem,
  ListPolishModelsResponse,
  PendingEditsResponse,
  PerformanceSnapshot,
  PolishDone,
  Preferences,
  PrefsUpdate,
  SttRuntimeInfo,
  Recording,
} from "../types";

// ── Mock history ──────────────────────────────────────────────────────────────

const now = Date.now();
const DAY = 86_400_000;

const MOCK_HISTORY: HistoryItem[] = [
  {
    timestamp_ms: now - 2 * 60 * 1000,
    polished: "User will need to install VP.",
    word_count: 7,
    recording_seconds: 3.2,
    model: "gpt-5.4",
    transcribe_ms: 420,
    embed_ms: 210,
    polish_ms: 610,
    audio_id: null,
    edit_count: 0,
  },
  {
    timestamp_ms: now - DAY - 2 * 60 * 60 * 1000,
    polished:
      "The analyze with AI button should only trigger on the existing DB metadata. The pull button had to do the detailed crawl.",
    word_count: 23,
    recording_seconds: 8.4,
    model: "gpt-5.4",
    transcribe_ms: 640,
    embed_ms: 290,
    polish_ms: 980,
    audio_id: null,
    edit_count: 0,
  },
  {
    timestamp_ms: now - DAY - 2 * 60 * 60 * 1000 - 60 * 1000,
    polished:
      "Yes, but this time we will get the whole tree, no? The earlier data was limited to only 1 page, so this time the whole tree will come and it will analyze the difference, right?",
    word_count: 38,
    recording_seconds: 11.1,
    model: "gpt-5.4",
    transcribe_ms: 710,
    embed_ms: 0,
    polish_ms: 890,
    audio_id: null,
    edit_count: 1,
  },
  {
    timestamp_ms: now - DAY - 9 * 60 * 60 * 1000,
    polished: "कि अभी तो मैं टाइप कर रहा हूं वैसे",
    word_count: 8,
    recording_seconds: 4.0,
    model: "gpt-5.4",
    transcribe_ms: 510,
    embed_ms: 180,
    polish_ms: 730,
    audio_id: null,
    edit_count: 0,
  },
  {
    timestamp_ms: now - DAY - 9 * 60 * 60 * 1000 - 3 * 60 * 1000,
    polished: "Theek hai.",
    word_count: 2,
    recording_seconds: 1.5,
    model: "gpt-5.4-mini",
    transcribe_ms: 280,
    embed_ms: 0,
    polish_ms: 330,
    audio_id: null,
    edit_count: 0,
  },
  {
    timestamp_ms: now - 2 * DAY - 3 * 60 * 60 * 1000,
    polished: "Can you check the latest deployment logs and see if there are any 5xx errors in the last hour?",
    word_count: 18,
    recording_seconds: 6.8,
    model: "claude-sonnet-4-6",
    transcribe_ms: 590,
    embed_ms: 240,
    polish_ms: 840,
    audio_id: null,
    edit_count: 0,
  },
  {
    timestamp_ms: now - 3 * DAY - 11 * 60 * 60 * 1000,
    polished: "Schedule a team sync for Thursday at 3 PM and share the agenda by Wednesday evening.",
    word_count: 16,
    recording_seconds: 5.9,
    model: "gpt-5.4",
    transcribe_ms: 480,
    embed_ms: 195,
    polish_ms: 700,
    audio_id: null,
    edit_count: 0,
  },
];

const MOCK_TOTAL_WORDS = MOCK_HISTORY.reduce((s, h) => s + h.word_count, 0) + 1132;
const MOCK_AVG_WPM = Math.round(
  MOCK_HISTORY.reduce((s, h) => s + h.word_count, 0) /
    MOCK_HISTORY.reduce((s, h) => s + h.recording_seconds / 60, 0)
);

// ── Mock snapshot ─────────────────────────────────────────────────────────────

const mockSnapshot: AppSnapshot = {
  state: "idle",
  platform: "browser-preview",
  current_mode: "mini",
  current_mode_label: "Fast (gpt-5.4-mini)",
  current_model: "gpt-5.4-mini",
  message_polish_mode: false,
  auto_paste_supported: false,
  accessibility_granted: false,
  microphone_granted: false,
  input_monitoring_granted: false,
  screen_recording_granted: true,
  modes: [
    { key: "mini", label: "Fast (gpt-5.4-mini)", model: "gpt-5.4-mini", icon: "fast" },
  ],
  last_result: {
    transcript: "kal sham meeting thodi delayed ho gayi thi",
    polished: "Kal sham meeting thodi delayed ho gayi thi.",
    model: "gpt-5.4",
    confidence: 0.94,
    transcribe_ms: 640,
    polish_ms: 980,
  },
  last_error: null,
  history: [...MOCK_HISTORY],
  total_words: MOCK_TOTAL_WORDS,
  daily_streak: 8,
  avg_wpm: MOCK_AVG_WPM || 186,
};

// ── Mock invoke ───────────────────────────────────────────────────────────────

async function mockInvoke(
  command: string,
  _args?: Record<string, unknown>
): Promise<AppSnapshot> {
  if (command === "bootstrap" || command === "request_accessibility") {
    return structuredClone(mockSnapshot);
  }

  if (
    command === "get_snapshot" ||
    command === "request_microphone" ||
    command === "request_input_monitoring"
  ) {
    return structuredClone(mockSnapshot);
  }

  if (command === "set_mode") {
    // Model switching removed — always mini
    return structuredClone(mockSnapshot);
  }

  if (command === "toggle_recording") {
    if (mockSnapshot.state === "idle") {
      mockSnapshot.state = "recording";
      return structuredClone(mockSnapshot);
    }

    // Simulate finish recording
    const newText = "Yeh draft thoda aur natural lagna chahiye.";
    const wordCount = newText.split(" ").length;
    const newItem: HistoryItem = {
      timestamp_ms: Date.now(),
      polished: newText,
      word_count: wordCount,
      recording_seconds: 4.5,
      model: mockSnapshot.current_model,
      transcribe_ms: 580,
      embed_ms: 0,
      polish_ms: 910,
      audio_id: null,
      edit_count: 0,
    };

    mockSnapshot.history = [newItem, ...mockSnapshot.history];
    mockSnapshot.total_words += wordCount;
    mockSnapshot.daily_streak = Math.max(mockSnapshot.daily_streak, 1);

    // Recalculate avg_wpm
    const recent = mockSnapshot.history.slice(0, 10);
    const totalW = recent.reduce((s, h) => s + h.word_count, 0);
    const totalM = recent.reduce((s, h) => s + h.recording_seconds / 60, 0);
    mockSnapshot.avg_wpm = totalM > 0 ? Math.round(totalW / totalM) : 186;

    mockSnapshot.state = "idle";
    mockSnapshot.last_result = {
      transcript: "yeh draft thoda aur natural lagna chahiye",
      polished: newText,
      model: mockSnapshot.current_model,
      confidence: 0.97,
      transcribe_ms: 580,
      polish_ms: 910,
    };
    return structuredClone(mockSnapshot);
  }

  throw new Error(`Unknown mock command: ${command}`);
}

// ── Tauri detection ───────────────────────────────────────────────────────────

export function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    (("__TAURI_INTERNALS__" in window && window.__TAURI_INTERNALS__ != null) ||
      ("__TAURI__" in window &&
        (window as Record<string, unknown>).__TAURI__ != null))
  );
}

// ── Public invoke ─────────────────────────────────────────────────────────────

export async function invoke<T = AppSnapshot>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  if (!isTauriRuntime()) {
    return mockInvoke(command, args) as Promise<T>;
  }
  return tauriInvoke<T>(command, args);
}

export async function requestMicrophone(): Promise<AppSnapshot> {
  return invoke<AppSnapshot>("request_microphone");
}


// ── Backend-aware commands (Phase E) ─────────────────────────────────────────

/** Get the local daemon URL + secret (for direct HTTP calls from the frontend). */
export async function getBackendEndpoint(): Promise<BackendEndpoint | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<BackendEndpoint>("get_backend_endpoint");
  } catch {
    return null;
  }
}

/** Fetch current user preferences from the backend. */
export async function getPreferences(): Promise<Preferences | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<Preferences>("get_preferences");
  } catch {
    return null;
  }
}

/** Active STT provider + env key presence (from Tauri process env). */
export async function getSttRuntime(): Promise<SttRuntimeInfo | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<SttRuntimeInfo>("get_stt_runtime");
  } catch {
    return null;
  }
}

export async function listPolishModels(
  beta = true
): Promise<ListPolishModelsResponse | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<ListPolishModelsResponse>("list_polish_models", {
      beta,
    });
  } catch {
    return null;
  }
}

/** Partially update preferences. Returns the updated preferences. */
export async function patchPreferences(
  update: PrefsUpdate,
  options?: { throwOnError?: boolean }
): Promise<Preferences | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<Preferences>("patch_preferences", { update });
  } catch (e) {
    if (options?.throwOnError) {
      throw e;
    }
    return null;
  }
}

// ── OpenAI / ChatGPT OAuth ───────────────────────────────────────────────────

export interface OpenAIStatus {
  connected: boolean;
  expires_at: number | null;
  connected_at: number | null;
}

export async function openaiConnect(): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<string>("openai_connect");
  } catch (e) {
    console.error("openai_connect failed:", e);
    return null;
  }
}

export async function openaiStatus(): Promise<OpenAIStatus | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<OpenAIStatus>("openai_status");
  } catch {
    return null;
  }
}

export async function openaiDisconnect(): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    await tauriInvoke("openai_disconnect");
    return true;
  } catch {
    return false;
  }
}

/** Fetch recording history from the backend (newest first). */
export async function listHistory(limit = 50, before?: number): Promise<Recording[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await tauriInvoke<Recording[]>("get_history", { limit, before: before ?? null });
  } catch {
    return [];
  }
}

/** Resolve a stored `target_app` (bundle-id on macOS / exe path on Windows) to a
 *  `data:image/png;base64,…` icon URL. Cached in the backend; `null` if unknown. */
export async function getAppIcon(appKey: string | null | undefined): Promise<string | null> {
  if (!isTauriRuntime() || !appKey || !appKey.trim()) return null;
  try {
    return await tauriInvoke<string | null>("get_app_icon", { appKey });
  } catch {
    return null;
  }
}

/** Resolve a `target_app` key to its full identity (name + category + icon). */
export async function getAppIdentity(appKey: string | null | undefined): Promise<AppIdentity | null> {
  if (!isTauriRuntime() || !appKey || !appKey.trim()) return null;
  try {
    return await tauriInvoke<AppIdentity | null>("get_app_identity", { appKey });
  } catch {
    return null;
  }
}

/** Per-app dictation usage (grouped by target_app, most-used first). */
export async function listAppUsage(): Promise<AppUsageRow[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await tauriInvoke<AppUsageRow[]>("get_app_usage");
  } catch {
    return [];
  }
}

/** Per-site dictation usage (grouped by host, most-used first). On-device only. */
export async function listSiteUsage(): Promise<SiteUsageRow[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await tauriInvoke<SiteUsageRow[]>("get_site_usage");
  } catch {
    return [];
  }
}

/** Favicon for a site host as a data: URL (direct fetch, cached). `null` → fallback. */
export async function getFavicon(host: string | null | undefined): Promise<string | null> {
  if (!isTauriRuntime() || !host || !host.trim()) return null;
  try {
    return await tauriInvoke<string | null>("get_favicon", { host });
  } catch {
    return null;
  }
}

export interface ProfileRunStats {
  run_count: number;
  skipped_count: number;
  last_run_at: string | null;
  last_run_outcome: string | null;
}

export interface KnowledgeBase {
  background: string | null;
  domains: string[];
  focus_areas: string[];
}

export interface BucketInsight {
  bucket_key: string;
  style: string[];
  speech_patterns: string[];
  version: number;
  updated_at: string | null;
}

export interface ProfileInsights {
  run_stats: ProfileRunStats;
  knowledge: KnowledgeBase;
  buckets: BucketInsight[];
}

/** What the cloud profiling brain has learned. `null` when signed out / offline. */
export async function getProfileInsights(): Promise<ProfileInsights | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<ProfileInsights>("get_profile_insights");
  } catch {
    return null;
  }
}

/** One app resolved to its bucket, for the Buckets kanban. */
export interface AppBucketRow {
  app_key: string;
  bucket_key: string;
  /** "user" | "static" | "agent" | "default" */
  source: string;
  count: number;
}

export interface AppBuckets {
  /** Canonical bucket keys in display order (the kanban columns). */
  buckets: string[];
  apps: AppBucketRow[];
  /** bucket_key -> output-language override, only for buckets that have one set. */
  bucket_languages?: Record<string, string>;
}

/** Apps grouped by bucket. `null` when signed out / offline. */
export async function getAppBuckets(): Promise<AppBuckets | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<AppBuckets>("get_app_buckets");
  } catch {
    return null;
  }
}

/** Re-file an app into a bucket (user override; wins over static + agent). */
export async function setAppBucket(appKey: string, bucketKey: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("set_app_bucket", { appKey, bucketKey });
}

/** Set (or clear, with `null`) the per-bucket output-language override. */
export async function setBucketLanguage(
  bucketKey: string,
  outputLanguage: string | null,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("set_bucket_language", { bucketKey, outputLanguage });
}

/** Diagnostic — try all 5 AX field-reading methods on whatever is focused. */
export interface AxMethodResult {
  method: string;
  label:  string;
  ok:     boolean;
  text:   string | null;
  err:    string | null;
}
export interface AxDiagnostics {
  ax_trusted:   boolean;
  app_name:     string | null;
  app_pid:      number | null;
  element_role: string | null;
  attributes:   string[];
  methods:      AxMethodResult[];
  clipboard:    string;
}

export interface DebugLogs {
  desktop_path: string;
  backend_path: string;
  desktop:      string;
  backend:      string;
  combined:     string;
  truncated:    boolean;
}

export async function getDebugLogs(): Promise<DebugLogs | null> {
  if (!isTauriRuntime()) {
    return {
      desktop_path: "~/Library/Logs/AirNote/said.log",
      backend_path: "~/Library/Logs/AirNote/backend.log",
      desktop:      "[main] said desktop starting — preview log",
      backend:      "airnote-backend build=0.1.0 features=openai_oauth+codex_api",
      combined:     "── AirNote desktop ──\n[main] airnote desktop starting — preview log\n\n── airnote-backend ──\nairnote-backend build=0.1.0 features=openai_oauth+codex_api",
      truncated:    false,
    };
  }
  try {
    return await tauriInvoke<DebugLogs>("get_debug_logs");
  } catch (e) {
    console.error("get_debug_logs failed", e);
    return null;
  }
}

export async function getPerformanceSnapshot(): Promise<PerformanceSnapshot | null> {
  if (!isTauriRuntime()) {
    const total = 16 * 1024 ** 3;
    const used = 7.5 * 1024 ** 3;
    return {
      timestamp_ms: Date.now(),
      cpu_percent: 18,
      physical_core_count: 8,
      total_memory_bytes: total,
      used_memory_bytes: used,
      available_memory_bytes: total - used,
      total_swap_bytes: 0,
      used_swap_bytes: 0,
      desktop: {
        pid: 101,
        name: "AirNote",
        cpu_percent: 3.4,
        memory_bytes: 240 * 1024 ** 2,
        virtual_memory_bytes: 0,
      },
      backend: {
        pid: 102,
        name: "airnote-backend",
        cpu_percent: 6.8,
        memory_bytes: 180 * 1024 ** 2,
        virtual_memory_bytes: 0,
      },
      gpu: {
        available: false,
        label: "GPU metrics unavailable from macOS user-space sampler",
        utilization_percent: null,
        memory_bytes: null,
      },
    };
  }
  try {
    return await tauriInvoke<PerformanceSnapshot>("get_performance_snapshot");
  } catch (e) {
    console.error("get_performance_snapshot failed", e);
    return null;
  }
}

export async function diagnoseAx(delaySecs: number): Promise<AxDiagnostics | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<AxDiagnostics>("diagnose_ax", { delaySecs });
  } catch (e) {
    console.error("diagnose_ax failed", e);
    return null;
  }
}

/** Open System Settings → Privacy & Security → Input Monitoring. */
export async function requestInputMonitoring(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("request_input_monitoring");
  } catch {
    // silently ignore
  }
}

/** Whether macOS Screen Recording is already granted (no prompt). */
export async function screenRecordingGranted(): Promise<boolean> {
  if (!isTauriRuntime()) return true;
  try {
    return await tauriInvoke<boolean>("screen_recording_granted");
  } catch {
    return false;
  }
}

/**
 * Ensure Screen Recording (needed for meeting system-audio capture): prompts +
 * opens the pane if missing. Returns the resulting grant state (often false
 * until the app is relaunched).
 */
export async function requestScreenRecording(): Promise<boolean> {
  if (!isTauriRuntime()) return true;
  try {
    return await tauriInvoke<boolean>("request_screen_recording");
  } catch {
    return false;
  }
}

/** Retry a recording by re-submitting its saved WAV. Result is auto-pasted. */
export async function retryRecording(audioId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("retry_recording", { audioId });
}

/** Delete a recording (SQLite row + WAV file). */
export async function deleteRecording(id: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("delete_recording", { id });
}

/** Return { url, secret } to fetch a recording's WAV audio with Authorization header. */
export async function getRecordingAudioUrl(
  id: string
): Promise<{ url: string; secret: string } | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<{ url: string; secret: string }>(
      "get_recording_audio_url", { id }
    );
  } catch {
    return null;
  }
}

/** Return WAV bytes for a recording. Keeps authenticated audio reads in Tauri. */
export async function getRecordingAudioBytes(id: string): Promise<Uint8Array | null> {
  if (!isTauriRuntime()) return null;
  try {
    const bytes = await tauriInvoke<number[] | Uint8Array>(
      "get_recording_audio_bytes", { id }
    );
    return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  } catch {
    return null;
  }
}

/** Save a recording WAV. Native app shows a save dialog and returns the saved path. */
export async function downloadRecordingAudio(
  id: string,
  filename: string
): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<string | null>("download_recording_audio", { id, filename });
  } catch {
    return null;
  }
}

/** Export History transcripts to a file. Native save dialog; returns the saved
 *  path, or null if cancelled / not in Tauri. Throws on write failure. */
export async function exportHistory(content: string, filename: string): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  return await tauriInvoke<string | null>("export_history", { content, filename });
}

export async function revealDownloadedFile(path: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("reveal_downloaded_file", { path });
}

/** Submit edit feedback so the backend can learn from user corrections. */
export async function submitEditFeedback(
  recordingId: string,
  userKept: string,
  targetApp?: string
): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("submit_edit_feedback", {
      recording_id: recordingId,
      user_kept: userKept,
      target_app: targetApp ?? null,
    });
  } catch {
    // Non-critical — swallow silently
  }
}

// ── SSE event listeners (Phase E streaming) ───────────────────────────────────

type Unsubscribe = () => void;

/** Listen for individual LLM tokens as they stream in. */
export function onVoiceToken(
  handler: (token: string) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<{ token: string }>("voice-token", (e) => handler(e.payload.token)).then(
    (fn) => { unsub = fn; }
  );
  return () => unsub();
}

/** Listen for status updates (transcribing / polishing). */
export function onVoiceStatus(
  handler: (phase: string, transcript?: string, runId?: string | null) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<{ phase: string; transcript?: string; run_id?: string | null; recording_id?: string | null }>("voice-status", (e) =>
    handler(e.payload.phase, e.payload.transcript, e.payload.run_id ?? e.payload.recording_id ?? null)
  ).then((fn) => { unsub = fn; });
  return () => unsub();
}

/** Listen for the final done event. */
export function onVoiceDone(
  handler: (done: PolishDone) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<PolishDone>("voice-done", (e) => handler(e.payload)).then(
    (fn) => { unsub = fn; }
  );
  return () => unsub();
}

/** Listen for error events. `audioId` is the saved WAV id for retrying. */
export type VoiceErrorPayload = {
  message: string;
  run_id?: string;
  audio_id?: string;
  error_code?: string;
  retryable?: boolean;
  owned_by_airnote?: boolean;
  diagnostic?: string;
  raw_error?: string;
};

export function onVoiceError(
  handler: (message: string, audioId?: string, errorCode?: string, payload?: VoiceErrorPayload) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<VoiceErrorPayload>("voice-error", (e) =>
    handler(e.payload.message, e.payload.audio_id, e.payload.error_code, e.payload)
  ).then((fn) => { unsub = fn; });
  return () => unsub();
}

/** Listen for detected edits that need user confirmation before being saved. */
export interface EditDetectedPayload {
  recording_id: string;
  ai_output:    string;
  user_kept:    string;
}
export function onEditDetected(
  handler: (payload: EditDetectedPayload) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<EditDetectedPayload>("edit-detected", (e) => handler(e.payload)).then(
    (fn) => { unsub = fn; }
  );
  return () => unsub();
}

/** Listen for app-state updates (e.g. state changed to processing/idle). */
export function onAppState(
  handler: (snap: AppSnapshot) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<AppSnapshot>("app-state", (e) => handler(e.payload)).then(
    (fn) => { unsub = fn; }
  );
  return () => unsub();
}

/** Listen for "nav-settings" — fired when the tray menu's Settings entry is clicked. */
export function onNavSettings(handler: (section?: string) => void): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<{ section?: string } | null>("nav-settings", (e) => handler(e.payload?.section)).then((fn) => { unsub = fn; });
  return () => unsub();
}

// ── Cloud auth commands ───────────────────────────────────────────────────────

/** Sign up for a new cloud account. Returns token + account info. */
export async function cloudSignup(
  email: string,
  password: string
): Promise<CloudAuthResponse> {
  if (!isTauriRuntime()) throw new Error("Tauri not available");
  return tauriInvoke<CloudAuthResponse>("cloud_signup", { email, password });
}

/** Log in to an existing cloud account. */
export async function cloudLogin(
  email: string,
  password: string
): Promise<CloudAuthResponse> {
  if (!isTauriRuntime()) throw new Error("Tauri not available");
  return tauriInvoke<CloudAuthResponse>("cloud_login", { email, password });
}

/** Log out (clears stored cloud token). */
export async function cloudLogout(): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke("cloud_logout");
}

/** Get current cloud connection status. */
export async function getCloudStatus(): Promise<CloudStatus | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<CloudStatus>("get_cloud_status");
  } catch {
    return null;
  }
}

// ── Notification permission ───────────────────────────────────────────────────

// isPermissionGranted() returns a PermissionState string: "granted" | "denied" | "prompt"
// IMPORTANT — Tauri plugin-notification surface:
//   isPermissionGranted()  returns Promise<boolean>      (NOT a string!)
//   requestPermission()    returns Promise<"granted"|"denied"|"default">
// The previous version cast the boolean as a string, so `true` never matched
// "granted" and the UI was permanently stuck on "Allow".

export type NotifPermission = "granted" | "denied" | "prompt" | "unknown";

/** Check the current macOS notification permission state without prompting. */
export async function checkNotificationPermission(): Promise<NotifPermission> {
  if (!isTauriRuntime()) return "unknown";
  try {
    const { isPermissionGranted } = await import("@tauri-apps/plugin-notification");
    const granted = await isPermissionGranted();
    if (granted === true)  return "granted";
    if (granted === false) return "prompt";
    return "unknown";
  } catch {
    return "unknown";
  }
}

/** Request macOS notification permission.
 *  Returns the resulting PermissionState string.
 *  NOTE: if already "denied", macOS will NOT re-prompt — user must enable in System Settings. */
export async function requestNotifications(): Promise<NotifPermission> {
  if (!isTauriRuntime()) return "unknown";
  try {
    const { isPermissionGranted, requestPermission } = await import(
      "@tauri-apps/plugin-notification"
    );
    if (await isPermissionGranted() === true) return "granted";
    // requestPermission returns "granted" | "denied" | "default"
    const result = await requestPermission();
    if (result === "granted") return "granted";
    if (result === "denied")  return "denied";
    return "prompt";   // "default" → still un-decided; treat as prompt-able
  } catch {
    return "unknown";
  }
}

/** Send a native notification. Silently no-ops if permission is not granted. */
export async function sendNotification(title: string, body: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const { isPermissionGranted, sendNotification: pluginSend } = await import(
      "@tauri-apps/plugin-notification"
    );
    // Tauri v2 plugin-notification's isPermissionGranted returns boolean, not
    // the string "granted" — the previous string check meant notifications
    // never fired.
    if (await isPermissionGranted()) {
      await pluginSend({ title, body });
    }
  } catch {
    // silently ignore
  }
}

// ── Pending-edit review ───────────────────────────────────────────────────────

export async function getPendingEdits(): Promise<PendingEditsResponse> {
  if (!isTauriRuntime()) return { edits: [], total: 0 };
  try {
    return await tauriInvoke<PendingEditsResponse>("get_pending_edits");
  } catch {
    return { edits: [], total: 0 };
  }
}

export async function resolvePendingEdit(
  id: string,
  action: "approve" | "skip"
): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("resolve_pending_edit", { id, action });
  } catch {
    // non-critical
  }
}

export async function dismissPendingEdit(id: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("dismiss_pending_edit", { id });
  } catch {
    // non-critical
  }
}

/** Listen for the backend's signal that pending edits list changed. */
export function onPendingEditsChanged(handler: () => void): () => void {
  if (!isTauriRuntime()) return () => {};
  let unsub: () => void = () => {};
  listen("pending-edits-changed", () => handler()).then((fn) => { unsub = fn; });
  return () => unsub();
}

// ── Vocabulary management ────────────────────────────────────────────────────

export interface VocabRow {
  term:            string;
  weight:          number;
  use_count:       number;
  last_used:       number;
  source:          "auto" | "manual" | "starred";
  meaning?:        string | null;
  term_type?:      string | null;
  example_context?: string | null;
}

export interface VocabListResponse {
  terms: VocabRow[];
  total: number;
}

export async function listVocabulary(): Promise<VocabListResponse> {
  if (!isTauriRuntime()) return { terms: [], total: 0 };
  try {
    return await tauriInvoke<VocabListResponse>("list_vocabulary");
  } catch {
    return { terms: [], total: 0 };
  }
}

/** A learned mishearing→canonical correction that rewrites dictation output. */
export interface VocabAlias {
  correct_form:    string;   // the canonical spelling (matches a vocab term)
  transcript_form: string;   // the mis-heard form STT produced
  use_count:       number;
  active:          boolean;  // fires at runtime (approved + not blocked)
}

export interface VocabAliasesResponse {
  aliases: VocabAlias[];
}

/** The real learned corrections behind vocab terms (stt_replacements). */
export async function listVocabularyAliases(): Promise<VocabAliasesResponse> {
  if (!isTauriRuntime()) return { aliases: [] };
  try {
    return await tauriInvoke<VocabAliasesResponse>("list_vocabulary_aliases");
  } catch {
    return { aliases: [] };
  }
}

export async function addVocabularyTerm(term: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("add_vocabulary_term", { term });
}

export async function deleteVocabularyTerm(term: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("delete_vocabulary_term", { term });
}

export async function resetAllVocabulary(): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("reset_all_vocabulary");
}

export async function patchVocabularyTerm(
  term: string,
  updates: { meaning?: string; term_type?: string; example_context?: string },
): Promise<void> {
  if (!isTauriRuntime()) return;
  await tauriInvoke("patch_vocabulary_term", {
    term,
    meaning: updates.meaning ?? null,
    termType: updates.term_type ?? null,
    exampleContext: updates.example_context ?? null,
  });
}

export async function starVocabularyTerm(term: string): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    return await tauriInvoke<boolean>("star_vocabulary_term", { term });
  } catch {
    return false;
  }
}

// ── External URL opener ─────────────────────────────────────────────────────

/**
 * Open a URL (https://, mailto:) in the OS default handler.
 * In a browser, falls back to `window.open` so dev mode in a normal browser
 * still works. In Tauri, calls the native opener so mailto: actually launches
 * the user's mail client (window.open silently fails in the webview).
 */
export async function openExternal(url: string): Promise<void> {
  if (!isTauriRuntime()) {
    window.open(url, "_blank");
    return;
  }
  try {
    await tauriInvoke("open_external", { url });
  } catch {
    // Last-ditch fallback so the user is never left with nothing happening.
    window.open(url, "_blank");
  }
}

// ── Invite a friend ─────────────────────────────────────────────────────────

export type InviteOutcome =
  | { status: "sent" }              // backend sent it via Resend
  | { status: "fallback_mailto" };  // backend has no email provider configured

/**
 * Try to send the invite email server-side via the backend.
 * Returns "fallback_mailto" if the backend has no Resend key configured —
 * the caller should then open `mailto:` so the user can send via their own client.
 * Throws on network/server errors.
 */
export async function sendInviteEmail(to: string): Promise<InviteOutcome> {
  if (!isTauriRuntime()) return { status: "fallback_mailto" };
  return await tauriInvoke<InviteOutcome>("send_invite_email", { to });
}

/** Listen for vocabulary mutations (manual add / delete / star toggle / auto-promote). */
export function onVocabularyChanged(handler: () => void): () => void {
  if (!isTauriRuntime()) return () => {};
  let unsub: () => void = () => {};
  listen("vocabulary-changed", () => handler()).then((fn) => { unsub = fn; });
  return () => unsub();
}

/** In-app vocabulary toast event payload (emitted by backend on add/promote/star/queue). */
export interface VocabToastPayload {
  /** "queued" — sighting recorded; k-event threshold not yet met (one more needed). */
  kind:   "added" | "starred" | "removed" | "queued";
  term:   string;
  source?: "auto" | "manual" | "starred";
}

export function onVocabToast(handler: (p: VocabToastPayload) => void): () => void {
  if (!isTauriRuntime()) return () => {};
  let unsub: () => void = () => {};
  listen<VocabToastPayload>("vocab-toast", (e) => handler(e.payload)).then((fn) => { unsub = fn; });
  return () => unsub();
}

/// Fired on launch when a dictation that was lost to a crash has been recovered
/// and re-transcribed. `text` is the recovered (polished) transcript.
export function onDictationRecovered(handler: (text: string) => void): () => void {
  if (!isTauriRuntime()) return () => {};
  let unsub: () => void = () => {};
  listen<{ text: string }>("dictation-recovered", (e) => handler(e.payload.text)).then((fn) => { unsub = fn; });
  return () => unsub();
}

// ── Desktop-only prefs (Sentry on/off + update channel) ───────────────────────
//
// These live in `<data_dir>/desktop_prefs.json` (NOT the backend's SQLite
// preferences DB) because they're read by the desktop process synchronously
// at startup, before the backend daemon is reachable. Changing them takes
// effect on next launch.
export interface DesktopPrefs {
  sentry_disabled: boolean;
  update_channel: "stable" | "beta";
  message_polish_mode: boolean;
  launch_at_login: boolean;
  beta_mode: boolean;
  browser_context_enabled: boolean;
}

export async function getDesktopPrefs(): Promise<DesktopPrefs> {
  if (!isTauriRuntime()) {
    return {
      sentry_disabled: false,
      update_channel: "stable",
      message_polish_mode: false,
      launch_at_login: false,
      beta_mode: false,
      browser_context_enabled: false,
    };
  }
  return tauriInvoke<DesktopPrefs>("get_desktop_prefs");
}

export async function setDesktopPrefs(prefs: DesktopPrefs): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("set_desktop_prefs", { prefs });
}

/** Prompt macOS Automation consent for running browsers (upfront, on Enable).
 *  Returns the browser names prompted. */
export async function requestBrowserAutomation(): Promise<string[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await tauriInvoke<string[]>("request_browser_automation");
  } catch {
    return [];
  }
}

export interface BrowserAutomation {
  app_key: string;
  name: string;
  running: boolean;
  status: "granted" | "denied" | "unknown";
}

/** Live macOS Automation consent state for every known browser currently
 *  running. Empty when no known browser is open (Apple Events need a live
 *  target), so the UI should tell the user to open their browser. */
export async function browserAutomationStatus(): Promise<BrowserAutomation[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await tauriInvoke<BrowserAutomation[]>("browser_automation_status");
  } catch {
    return [];
  }
}

/** Fire the Automation consent dialog for one specific browser (by bundle-id). */
export async function triggerBrowserAutomation(appKey: string): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    return await tauriInvoke<boolean>("trigger_browser_automation", { appKey });
  } catch {
    return false;
  }
}

// ── Developer Problem Command ────────────────────────────────────────────────

export interface DeveloperProjectProfile {
  id: string;
  name: string;
  aliases: string[];
  context: string;
  enabled: boolean;
  source_type: string;
  updated_at: number;
}

export interface DeveloperSettings {
  enabled: boolean;
  command_key: string;
  profiles: DeveloperProjectProfile[];
}

export interface DeveloperProfileWarning {
  profile_id: string;
  alias: string | null;
  message: string;
}

export interface DeveloperSettingsResponse {
  settings: DeveloperSettings;
  warnings: DeveloperProfileWarning[];
}

export interface DeveloperContextCandidate {
  id: string;
  name: string;
  matched_alias: string;
}

export interface DeveloperContextEvent {
  outcome: "project" | "none" | "ambiguous";
  label: string;
  project: DeveloperContextCandidate | null;
  candidates: DeveloperContextCandidate[];
}

export function emptyDeveloperSettings(): DeveloperSettings {
  return {
    enabled: false,
    command_key: "tray",
    profiles: [],
  };
}

export async function getDeveloperSettings(): Promise<DeveloperSettingsResponse> {
  if (!isTauriRuntime()) {
    return { settings: emptyDeveloperSettings(), warnings: [] };
  }
  return tauriInvoke<DeveloperSettingsResponse>("developer_get_settings");
}

export async function saveDeveloperSettings(
  settings: DeveloperSettings
): Promise<DeveloperSettingsResponse> {
  if (!isTauriRuntime()) {
    return { settings, warnings: [] };
  }
  return tauriInvoke<DeveloperSettingsResponse>("developer_save_settings", { settings });
}

export async function developerProblemBegin(): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("developer_problem_begin");
}

export async function developerProblemEnd(): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("developer_problem_end");
}

export async function developerProblemChooseProject(projectId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("developer_problem_choose_project", { projectId });
}

export async function developerProblemDismiss(): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("developer_problem_dismiss");
}

export function onDeveloperContext(
  handler: (payload: DeveloperContextEvent) => void
): Unsubscribe {
  if (!isTauriRuntime()) return () => {};
  let unsub: Unsubscribe = () => {};
  listen<DeveloperContextEvent>("problem-command-context", (e) => handler(e.payload))
    .then((fn) => { unsub = fn; });
  return () => unsub();
}

// ── Developer log (backend.log tail) ─────────────────────────────────────────

export async function readBackendLog(maxLines = 600): Promise<string> {
  if (!isTauriRuntime()) return "(dev log only available in the desktop app)";
  return tauriInvoke<string>("read_backend_log", { maxLines });
}

export async function backendLogLocation(): Promise<string> {
  if (!isTauriRuntime()) return "";
  return tauriInvoke<string>("backend_log_location");
}

export async function openLogFolder(): Promise<void> {
  if (!isTauriRuntime()) return;
  return tauriInvoke<void>("open_log_folder");
}

// ── Divo (Ctrl hold-to-talk → agent) ──────────────────────────────────────────

export type DivoStatusPayload = {
  liveLabel?: string;
  progressPct?: number;
  phase?: string;
  plan?: { status: string; title: string; subtitle?: string }[];
};
export type DivoToolPayload = {
  phase: "start" | "end";
  name: string;
  family?: string | null;
  verb?: string | null;
  past?: string | null;
  ok?: boolean;
  callId?: string | null;
};

/** Push the control-plane URL + session token to Rust (enables the Ctrl hotkey). */
export async function divoSetCredentials(serverUrl: string, token: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("divo_set_credentials", { serverUrl, token });
  } catch (e) {
    console.warn("[divo] set_credentials failed", e);
  }
}

/** Panel "Speak follow-up" — press-and-hold record on the active thread. */
export async function divoFollowupBegin(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("divo_followup_begin");
  } catch (e) {
    console.warn("[divo] followup_begin failed", e);
  }
}
export async function divoFollowupEnd(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("divo_followup_end");
  } catch (e) {
    console.warn("[divo] followup_end failed", e);
  }
}

/** Recover the latest assistant answer for a thread (post-disconnect / approval). */
export async function divoFetchThread(threadId: string): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await tauriInvoke<string | null>("divo_fetch_thread", { threadId });
  } catch (e) {
    console.warn("[divo] fetch_thread failed", e);
    return null;
  }
}

// ── Divo chats (in-app history + HUD chat router) ─────────────────────────────

export type DivoThreadSummary = {
  id: string;
  title: string;
  createdAt?: string;
  updatedAt?: string;
  lastMessageAt?: string | null;
  preview?: string;
};

export type DivoMessage = {
  id: string;
  threadId: string;
  role: "user" | "assistant";
  content: string;
  createdAt: string;
};

export type DivoThreadDetail = {
  id: string;
  title: string;
  createdAt?: string;
  updatedAt?: string;
  lastMessageAt?: string | null;
  messages: DivoMessage[];
};

/** Send a reviewed instruction to a chat (`threadId`) or a new one (`null`). */
export async function divoSend(message: string, threadId: string | null): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("divo_send", { message, threadId });
  } catch (e) {
    console.warn("[divo] send failed", e);
  }
}

/** List the user's AirNote Divo chats (most recent first). */
export async function divoListThreads(): Promise<DivoThreadSummary[]> {
  if (!isTauriRuntime()) return [];
  try {
    const res = await tauriInvoke<{ data?: { threads?: DivoThreadSummary[] } }>("divo_list_threads");
    return res?.data?.threads ?? [];
  } catch (e) {
    console.warn("[divo] list_threads failed", e);
    return [];
  }
}

/** Fetch a full thread (all messages) for the in-app conversation pane. */
export async function divoThreadMessages(threadId: string): Promise<DivoThreadDetail | null> {
  if (!isTauriRuntime()) return null;
  try {
    const res = await tauriInvoke<{ data?: DivoThreadDetail }>("divo_thread_messages", { threadId });
    return res?.data ?? null;
  } catch (e) {
    console.warn("[divo] thread_messages failed", e);
    return null;
  }
}

/** Mark a chat as active (Ctrl continues it), or clear it (`null` → next Ctrl = new). */
export async function divoSetActiveThread(threadId: string | null): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await tauriInvoke("divo_set_active_thread", { threadId });
  } catch (e) {
    console.warn("[divo] set_active_thread failed", e);
  }
}

function divoListener<T>(event: string, handler: (p: T) => void): () => void {
  if (!isTauriRuntime()) return () => {};
  let unsub: () => void = () => {};
  listen<T>(event, (e) => handler(e.payload)).then((fn) => {
    unsub = fn;
  });
  return () => unsub();
}

export const onDivoStarted = (h: (p: { followup: boolean }) => void) =>
  divoListener("divo-started", h);
export const onDivoMeta = (h: (p: { threadId: string }) => void) =>
  divoListener("divo-meta", h);
export const onDivoStatus = (h: (p: DivoStatusPayload) => void) =>
  divoListener("divo-status", h);
export const onDivoThinking = (h: (p: { text: string }) => void) =>
  divoListener("divo-thinking", h);
export const onDivoTool = (h: (p: DivoToolPayload) => void) =>
  divoListener("divo-tool", h);
export const onDivoDone = (h: (p: { content: string; threadId: string | null }) => void) =>
  divoListener("divo-done", h);
export const onDivoError = (h: (p: { message: string }) => void) =>
  divoListener("divo-error", h);
export const onDivoPending = (h: (p: { message: string }) => void) =>
  divoListener("divo-pending", h);

// ── Server migration ──────────────────────────────────────────────────────────

export interface ServerMigrationStatus {
  status: "not_started" | "running" | "partial" | "completed" | "failed";
  migration_version: number;
  uploaded_history_count: number;
  uploaded_vocab_count: number;
  uploaded_alias_count: number;
  uploaded_email_count: number;
  uploaded_credentials_count: number;
  last_error?: string | null;
  last_attempt_at_ms?: number | null;
  completed_at_ms?: number | null;
  server_url?: string | null;
  signed_in: boolean;
}

async function backendFetch(path: string, opts: RequestInit = {}): Promise<Response | null> {
  try {
    const endpoint = await getBackendEndpoint();
    if (!endpoint?.url || !endpoint.secret) return null;
    const headers: Record<string, string> = {
      ...(opts.headers as Record<string, string> | undefined),
      Authorization: `Bearer ${endpoint.secret}`,
    };
    if (opts.body && !(headers["Content-Type"])) headers["Content-Type"] = "application/json";
    return fetch(`${endpoint.url}${path}`, { ...opts, headers });
  } catch {
    return null;
  }
}

export async function getMigrationStatus(): Promise<ServerMigrationStatus | null> {
  try {
    const res = await backendFetch("/v1/server-migration/status");
    if (!res?.ok) return null;
    return await res.json() as ServerMigrationStatus;
  } catch {
    return null;
  }
}

export async function runMigration(): Promise<{ started: boolean; reason?: string } | null> {
  try {
    const res = await backendFetch("/v1/server-migration/run", { method: "POST", body: "{}" });
    if (!res) return null;
    return await res.json() as { started: boolean; reason?: string };
  } catch {
    return null;
  }
}

export async function cancelMigration(): Promise<void> {
  try {
    await backendFetch("/v1/server-migration/cancel", { method: "POST", body: "{}" });
  } catch {
    // best-effort
  }
}

// ── Server settings sync ──────────────────────────────────────────────────────

export interface ServerSettingsStatus {
  synced: boolean;
  server_version: number;
  last_synced_at_ms?: number | null;
  last_error?: string | null;
  settings?: Record<string, unknown> | null;
  signed_in: boolean;
}

export async function getServerSettingsStatus(): Promise<ServerSettingsStatus | null> {
  try {
    const res = await backendFetch("/v1/server-settings/status");
    if (!res?.ok) return null;
    return await res.json() as ServerSettingsStatus;
  } catch {
    return null;
  }
}

export async function syncServerSettings(): Promise<{ synced: boolean; reason?: string } | null> {
  try {
    const res = await backendFetch("/v1/server-settings/sync", { method: "POST", body: "{}" });
    if (!res) return null;
    return await res.json() as { synced: boolean; reason?: string };
  } catch {
    return null;
  }
}

// ── Runtime credential vault (local keys → server DB) ─────────────────────────

export interface RuntimeCredentialSummary {
  id: string;
  provider: string;
  scope: string;
  display_name: string;
  secret_last4: string;
  status: string;
  updated_at?: string | null;
}

export interface CredentialVaultStatus {
  signed_in: boolean;
  server_url?: string | null;
  encryption_configured: boolean;
  server_credentials: RuntimeCredentialSummary[];
  local_providers: string[];
}

export interface CredentialSyncResult {
  provider: string;
  action: string;
  error?: string | null;
}

export interface CredentialSyncResponse {
  connected: boolean;
  server_url?: string | null;
  attempted: number;
  synced: number;
  skipped: number;
  failed: number;
  revoked: number;
  reason?: string | null;
  results?: CredentialSyncResult[];
}

export async function getCredentialVaultStatus(): Promise<CredentialVaultStatus | null> {
  try {
    const res = await backendFetch("/v1/runtime/credentials/status");
    if (!res?.ok) return null;
    return await res.json() as CredentialVaultStatus;
  } catch {
    return null;
  }
}

export async function syncCredentialVault(): Promise<CredentialSyncResponse | null> {
  try {
    const res = await backendFetch("/v1/runtime/credentials/sync", {
      method: "POST",
      body: "{}",
    });
    if (!res) return null;
    return await res.json() as CredentialSyncResponse;
  } catch {
    return null;
  }
}

// Suppress unused-import warnings for types only used in exported signatures
export type {
  CloudAuthResponse,
  CloudStatus,
  HistoryItem,
  PendingEditsResponse,
  PolishDone,
  Preferences,
  PrefsUpdate,
  Recording,
  BackendEndpoint,
};
