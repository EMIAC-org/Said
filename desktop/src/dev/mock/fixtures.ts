/**
 * Mock data for the browser preview (`npm run mock`). Everything here is typed
 * against the app's own interfaces, so a changed shape fails `npm run typecheck`
 * instead of quietly rendering a broken screen.
 */
import type {
  AppBuckets,
  DesktopPrefs,
  LocalModelInfo,
  LocalModelInventory,
  ProfileInsights,
  SttSetupPolicy,
  VocabAlias,
  VocabRow,
} from "@/lib/invoke";
import type {
  AppIdentity,
  AppSnapshot,
  AppUsageRow,
  PerformanceSnapshot,
  Preferences,
  Recording,
  SiteUsageRow,
  SttRuntimeInfo,
} from "@/types";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export const MOCK_EMAIL = "abhishek@emiactech.com";
export const MOCK_ORG = "EMIAC Technologies";

/** Apps the history rows are dictated into, with the name and brand colour the
 *  mock icon uses. Keys are real macOS bundle ids, as the backend stores them. */
export const MOCK_APPS: Record<string, { name: string; category: string; color: string }> = {
  "com.tinyspeck.slackmacgap": { name: "Slack", category: "messaging", color: "#4A154B" },
  "com.google.Chrome": { name: "Google Chrome", category: "default", color: "#1A73E8" },
  "com.apple.mail": { name: "Mail", category: "formal_writing", color: "#1E88E5" },
  "notion.id": { name: "Notion", category: "work_tracker", color: "#191919" },
  "com.microsoft.VSCode": { name: "Visual Studio Code", category: "coding", color: "#0065A9" },
  "net.whatsapp.WhatsApp": { name: "WhatsApp", category: "messaging", color: "#25D366" },
  "com.linear": { name: "Linear", category: "work_tracker", color: "#5E6AD2" },
  "com.apple.Notes": { name: "Notes", category: "default", color: "#F5B400" },
};

type Sample = { app: keyof typeof MOCK_APPS; raw: string; polished: string };

/** Hinglish in, clean text out: the product's actual job, so every screen that
 *  shows history shows the realistic mix. */
const SAMPLES: Sample[] = [
  { app: "com.tinyspeck.slackmacgap", raw: "haan main kal subah tak PR review kar dunga", polished: "Haan, main kal subah tak PR review kar dunga." },
  { app: "com.apple.mail", raw: "hi rahul please find attached the revised proposal let me know if the pricing works for you", polished: "Hi Rahul,\n\nPlease find attached the revised proposal. Let me know if the pricing works for you." },
  { app: "com.microsoft.VSCode", raw: "todo retry the upload with exponential backoff max five attempts", polished: "TODO: retry the upload with exponential backoff, max five attempts." },
  { app: "notion.id", raw: "launch checklist landing page update changelog aur release notes bhejna hai", polished: "Launch checklist: landing page update, changelog, aur release notes bhejna hai." },
  { app: "net.whatsapp.WhatsApp", raw: "bhai main raaste mein hoon das minute mein pahunchta hoon", polished: "Bhai, main raaste mein hoon — das minute mein pahunchta hoon." },
  { app: "com.linear", raw: "history view is empty when polish is off because the recording never gets saved", polished: "History view is empty when polish is off, because the recording never gets saved." },
  { app: "com.google.Chrome", raw: "whisper cpp metal backend benchmark on m two air", polished: "whisper.cpp Metal backend benchmark on M2 Air" },
  { app: "com.tinyspeck.slackmacgap", raw: "deploy ho gaya hai prod par ek baar check kar lo", polished: "Deploy ho gaya hai prod par, ek baar check kar lo." },
  { app: "com.apple.Notes", raw: "groceries doodh bread anda aur coffee beans", polished: "Groceries: doodh, bread, anda aur coffee beans." },
  { app: "com.apple.mail", raw: "thanks for the quick turnaround we will share the signed copy by friday", polished: "Thanks for the quick turnaround. We will share the signed copy by Friday." },
  { app: "com.microsoft.VSCode", raw: "this function returns none when the marker file is missing so treat it as legacy", polished: "This function returns None when the marker file is missing, so treat it as legacy." },
  { app: "notion.id", raw: "meeting notes client wants onboarding in two steps not four", polished: "Meeting notes: client wants onboarding in two steps, not four." },
  { app: "com.linear", raw: "light mode ka pura revamp karna hai emiac site ke colours ke saath", polished: "Light mode ka pura revamp karna hai, EMIAC site ke colours ke saath." },
  { app: "net.whatsapp.WhatsApp", raw: "haan theek hai sham ko call karte hain", polished: "Haan, theek hai — sham ko call karte hain." },
];

function recording(index: number, sample: Sample, timestamp: number): Recording {
  const words = sample.polished.split(/\s+/).filter(Boolean).length;
  const polishOff = index % 11 === 7;
  return {
    id: `mock-rec-${index}`,
    timestamp_ms: timestamp,
    transcript: sample.raw,
    polished: polishOff ? sample.raw : sample.polished,
    final_text: null,
    word_count: words,
    recording_seconds: Math.max(2, Math.round((words / 2.6) * 10) / 10),
    model_used: polishOff ? "polish_disabled" : "gemma-3-27b",
    confidence: 0.93,
    transcribe_ms: 380 + ((index * 37) % 240),
    embed_ms: 0,
    polish_ms: polishOff ? 0 : 620 + ((index * 53) % 400),
    target_app: sample.app,
    edit_count: index % 5 === 2 ? 1 : 0,
    source: "voice",
    audio_id: null,
    enriched_transcript: null,
    raw_transcript: sample.raw,
    local_corrected_transcript: null,
    polished_output: polishOff ? null : sample.polished,
  };
}

/** About three weeks of use: a few dictations most days, busier on weekdays. */
export function buildHistory(now = Date.now()): Recording[] {
  const rows: Recording[] = [];
  let index = 0;
  for (let day = 0; day < 20; day += 1) {
    const perDay = day === 0 ? 5 : [3, 4, 2, 5, 1, 0, 3][day % 7];
    for (let slot = 0; slot < perDay; slot += 1) {
      const sample = SAMPLES[index % SAMPLES.length];
      const timestamp = now - day * DAY - (slot * 97 + 4) * MINUTE;
      rows.push(recording(index, sample, timestamp));
      index += 1;
    }
  }
  return rows;
}

export function snapshot(history: Recording[], granted: boolean): AppSnapshot {
  const totalWords = history.reduce((sum, row) => sum + row.word_count, 0);
  return {
    state: "idle",
    platform: "macos",
    current_mode: "mini",
    current_mode_label: "Polish",
    current_model: "gemma-3-27b",
    message_polish_mode: false,
    auto_paste_supported: true,
    accessibility_granted: granted,
    microphone_granted: granted,
    input_monitoring_granted: granted,
    screen_recording_granted: granted,
    modes: [{ key: "mini", label: "Polish", model: "gemma-3-27b", icon: "fast" }],
    last_result: null,
    last_error: null,
    history: [],
    total_words: totalWords,
    daily_streak: history.length ? 6 : 0,
    avg_wpm: history.length ? 148 : 0,
  };
}

export const preferences: Preferences = {
  user_id: "mock-user",
  selected_model: "gemma-3-27b",
  tone_preset: "natural",
  custom_prompt: null,
  language: "auto",
  output_language: "hinglish",
  auto_paste: true,
  edit_capture: true,
  polish_text_hotkey: "",
  record_hotkey: "CapsLock",
  learning_enabled: true,
  polish_enabled: true,
  server_runtime_enabled: true,
  server_audio_runtime_enabled: false,
  gateway_api_key: null,
  gemini_api_key: null,
  groq_api_key: null,
  deepinfra_api_key: null,
  llm_provider: "server",
};

export const desktopPrefs: DesktopPrefs = {
  sentry_disabled: false,
  update_channel: "stable",
  message_polish_mode: false,
  launch_at_login: true,
  beta_mode: false,
  browser_context_enabled: true,
  dictation_stt: "local",
  local_stt_model: "clario-hinglish-41h",
  local_stt_compat_override: null,
};

export const sttPolicy: SttSetupPolicy = {
  platform: "macos",
  cpu_family: "apple_silicon",
  total_memory_bytes: 16 * 1024 ** 3,
  setup_kind: "local_required",
  local_model: "clario-hinglish-41h",
  local_model_name: "AirNote Hinglish (41h)",
  local_model_size_hint: "~141 MB",
};

export const sttRuntime: SttRuntimeInfo = {
  dictation_provider: "local-whisper",
  dictation_ready: true,
  dictation_stt_pref: "local",
  dictation_auto_provider: "local-whisper",
  whisper_installed: true,
  whisper_ready: true,
  whisper_vad_installed: true,
  local_stt_model: "clario-hinglish-41h",
};

/** `current`: the 41h model is installed and selected. `legacy`: only the old
 *  Oriserve file is on disk, which is what triggers the update gate. */
export function modelInventory(state: "current" | "legacy" | "none"): LocalModelInventory {
  const current: LocalModelInfo = {
    key: "clario-hinglish-41h",
    name: "AirNote Hinglish (41h)",
    installed: state === "current",
    size_bytes: 147_951_465,
    size_hint: "~141 MB",
    recommended: true,
    active_for_dictation: state === "current",
    required_for_meetings: false,
    compatibility_candidate: false,
    safe_to_remove: false,
  };
  const legacy: LocalModelInfo = {
    key: "oriserve",
    name: "Oriserve Hinglish (legacy)",
    installed: state === "legacy",
    size_bytes: 155_000_000,
    size_hint: "~148 MB",
    recommended: false,
    active_for_dictation: state === "legacy",
    required_for_meetings: false,
    compatibility_candidate: true,
    safe_to_remove: true,
  };
  return {
    setup_kind: "local_required",
    recommended_model: "clario-hinglish-41h",
    selected_model: state === "legacy" ? "oriserve" : "clario-hinglish-41h",
    recommended_installed: state === "current",
    existing_compatible_model: state === "legacy" ? "oriserve" : null,
    models: [current, legacy],
    reclaimable_bytes: 0,
  };
}

export const vocabulary: VocabRow[] = [
  { term: "AirNote", weight: 1, use_count: 64, last_used: Date.now() - HOUR, source: "starred", meaning: "This app", term_type: "product" },
  { term: "EMIAC", weight: 1, use_count: 41, last_used: Date.now() - 3 * HOUR, source: "manual", meaning: "The company", term_type: "org" },
  { term: "Clario", weight: 0.9, use_count: 28, last_used: Date.now() - DAY, source: "auto", term_type: "product" },
  { term: "whisper.cpp", weight: 0.8, use_count: 17, last_used: Date.now() - 2 * DAY, source: "auto", term_type: "tech" },
  { term: "Tauri", weight: 0.8, use_count: 12, last_used: Date.now() - 2 * DAY, source: "auto", term_type: "tech" },
  { term: "Lark", weight: 0.7, use_count: 9, last_used: Date.now() - 4 * DAY, source: "auto", term_type: "product" },
  { term: "Hinglish", weight: 0.7, use_count: 22, last_used: Date.now() - 5 * HOUR, source: "starred", term_type: "language" },
  { term: "Rahul", weight: 0.6, use_count: 6, last_used: Date.now() - 6 * DAY, source: "auto", term_type: "person" },
];

export const vocabAliases: VocabAlias[] = [
  { correct_form: "AirNote", transcript_form: "air note", use_count: 31, active: true },
  { correct_form: "EMIAC", transcript_form: "e mac", use_count: 12, active: true },
  { correct_form: "whisper.cpp", transcript_form: "whisper cpp", use_count: 7, active: true },
];

export const profileInsights: ProfileInsights = {
  run_stats: { run_count: 14, skipped_count: 2, last_run_at: new Date(Date.now() - 2 * HOUR).toISOString(), last_run_outcome: "updated" },
  knowledge: {
    background: "Builds desktop and AI products at EMIAC; writes in a mix of English and Hinglish.",
    domains: ["Speech recognition", "Desktop apps", "Product launches"],
    focus_areas: ["AirNote releases", "Model quality", "Onboarding"],
  },
  buckets: [
    { bucket_key: "messaging", style: ["Short, casual lines", "Hinglish kept in Roman script"], speech_patterns: ["Starts with 'haan' or 'bhai'"], version: 4, updated_at: new Date(Date.now() - DAY).toISOString() },
    { bucket_key: "formal_writing", style: ["Full sentences", "Greeting and sign-off"], speech_patterns: ["Dictates the salutation first"], version: 2, updated_at: new Date(Date.now() - 3 * DAY).toISOString() },
    { bucket_key: "coding", style: ["Technical terms kept verbatim"], speech_patterns: ["Uses TODO prefixes"], version: 3, updated_at: new Date(Date.now() - 2 * DAY).toISOString() },
  ],
};

export function appBuckets(history: Recording[]): AppBuckets {
  const counts = new Map<string, number>();
  for (const row of history) if (row.target_app) counts.set(row.target_app, (counts.get(row.target_app) ?? 0) + 1);
  return {
    buckets: ["coding", "messaging", "work_tracker", "formal_writing", "default"],
    apps: [...counts].map(([app_key, count]) => ({
      app_key,
      bucket_key: MOCK_APPS[app_key]?.category ?? "default",
      source: "auto",
      count,
    })),
  };
}

export function appUsage(history: Recording[]): AppUsageRow[] {
  const rows = new Map<string, AppUsageRow>();
  for (const row of history) {
    if (!row.target_app) continue;
    const entry = rows.get(row.target_app) ?? { app: row.target_app, count: 0, total_words: 0, last_used_ms: 0 };
    entry.count += 1;
    entry.total_words += row.word_count;
    entry.last_used_ms = Math.max(entry.last_used_ms, row.timestamp_ms);
    rows.set(row.target_app, entry);
  }
  return [...rows.values()].sort((a, b) => b.count - a.count);
}

export const siteUsage: SiteUsageRow[] = [
  { host: "mail.google.com", target_app: "com.google.Chrome", count: 9, last_used_ms: Date.now() - 2 * HOUR },
  { host: "github.com", target_app: "com.google.Chrome", count: 6, last_used_ms: Date.now() - 5 * HOUR },
  { host: "linear.app", target_app: "com.google.Chrome", count: 4, last_used_ms: Date.now() - DAY },
];

export function appIdentity(key: string): AppIdentity | null {
  const app = MOCK_APPS[key];
  return app ? { key, name: app.name, category: app.category, icon: null } : null;
}

/** A rounded-square letter tile in the app's brand colour, so lists that show
 *  app icons look populated without shipping real icons. */
export function appIcon(key: string): string | null {
  const app = MOCK_APPS[key];
  if (!app) return null;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" rx="14" fill="${app.color}"/><text x="32" y="42" font-family="-apple-system,Helvetica,sans-serif" font-size="30" font-weight="600" fill="#fff" text-anchor="middle">${app.name[0]}</text></svg>`;
  return `data:image/svg+xml;base64,${btoa(svg)}`;
}

export function performance(): PerformanceSnapshot {
  const gb = 1024 ** 3;
  return {
    timestamp_ms: Date.now(),
    cpu_percent: 6 + Math.random() * 4,
    physical_core_count: 8,
    total_memory_bytes: 16 * gb,
    used_memory_bytes: 9.4 * gb,
    available_memory_bytes: 6.6 * gb,
    total_swap_bytes: 2 * gb,
    used_swap_bytes: 0.3 * gb,
    desktop: { pid: 4101, name: "AirNote", cpu_percent: 1.2, memory_bytes: 182 * 1024 ** 2, virtual_memory_bytes: 1.2 * gb },
    backend: { pid: 4102, name: "airnote-backend", cpu_percent: 0.4, memory_bytes: 96 * 1024 ** 2, virtual_memory_bytes: 0.8 * gb },
    gpu: { available: true, label: "Apple M2", utilization_percent: 3, memory_bytes: null },
  };
}
