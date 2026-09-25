/**
 * Browser preview for the AirNote UI: `npm run mock` (or `just mock`).
 *
 * Vite injects this module ahead of `main.tsx`, and only in `--mode mock`, so a
 * release build never contains it. It stands in for the two things the UI talks
 * to: the Tauri IPC bridge (every command and event) and HTTP (the local backend
 * and the control plane). The real app code runs unchanged on top.
 *
 * URL parameters:
 *   ?scenario=ready | new | onboarding | update | signed-out
 *   ?theme=light | dark
 * Both stick across reloads; the switcher in the bottom-right corner sets them.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Recording } from "@/types";
import * as fx from "./fixtures";

export const SCENARIOS = {
  ready: "Signed in, three weeks of history",
  new: "Signed in, nothing dictated yet",
  onboarding: "First launch: onboarding",
  update: "Existing user: model update gate",
  "signed-out": "Onboarded but signed out",
} as const;
type Scenario = keyof typeof SCENARIOS;

const SCENARIO_KEY = "said:mock-scenario";
const MIGRATION_VERSION_DONE = "8";
const MOCK_SERVER = "https://airnote.mock";

function readScenario(): Scenario {
  const params = new URLSearchParams(location.search);
  const asked = params.get("scenario") ?? localStorage.getItem(SCENARIO_KEY) ?? "ready";
  return (asked in SCENARIOS ? asked : "ready") as Scenario;
}

/** A scenario is a starting state. It is written once when the scenario changes,
 *  so what you do inside it (finish onboarding, sign in) survives a reload. */
function seedStorage(scenario: Scenario) {
  if (localStorage.getItem(SCENARIO_KEY) === scenario) return;
  const theme = localStorage.getItem("vp-theme");
  localStorage.clear();
  if (theme) localStorage.setItem("vp-theme", theme);
  localStorage.setItem(SCENARIO_KEY, scenario);
  if (scenario === "onboarding") return;
  localStorage.setItem("said:onboarding-complete", "true");
  localStorage.setItem("said:migration-done", scenario === "update" ? "7" : MIGRATION_VERSION_DONE);
  if (scenario !== "signed-out") {
    localStorage.setItem(
      "said:enterprise",
      JSON.stringify({ serverUrl: MOCK_SERVER, jwt: "mock-jwt", accountId: "mock-account", email: fx.MOCK_EMAIL, orgName: fx.MOCK_ORG, authSource: "email" }),
    );
  }
}

function applyTheme() {
  const theme = new URLSearchParams(location.search).get("theme");
  if (theme === "light" || theme === "dark") {
    localStorage.setItem("vp-theme", theme);
    document.documentElement.dataset.theme = theme;
  }
}

// ── State the commands read and change ──────────────────────────────────────

const scenario = readScenario();
applyTheme();
seedStorage(scenario);

const state = {
  history: scenario === "new" || scenario === "onboarding" ? [] : fx.buildHistory(),
  granted: scenario !== "onboarding",
  model: (scenario === "update" ? "legacy" : scenario === "onboarding" ? "none" : "current") as "current" | "legacy" | "none",
  recording: "idle" as "idle" | "recording" | "processing",
  preferences: { ...fx.preferences },
  desktopPrefs: { ...fx.desktopPrefs },
  dictionary: scenario === "new" || scenario === "onboarding" ? [] : [...fx.dictionary],
  downloadCancelled: false,
};

function snapshot() {
  return { ...fx.snapshot(state.history, state.granted), state: state.recording };
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** A dictation: recording while the key is held, a short "processing", then a
 *  new history row and the same events the real backend sends. */
async function toggleRecording() {
  if (state.recording === "idle") {
    state.recording = "recording";
    void emit("app-state", snapshot());
    return snapshot();
  }
  state.recording = "processing";
  void emit("app-state", snapshot());
  await sleep(700);
  const polished = "Ye mock dictation hai — browser preview se aayi hai.";
  const row: Recording = {
    ...fx.buildHistory()[0],
    id: `mock-live-${Date.now()}`,
    timestamp_ms: Date.now(),
    transcript: "ye mock dictation hai browser preview se aayi hai",
    polished,
    polished_output: polished,
    word_count: 8,
    target_app: "com.tinyspeck.slackmacgap",
  };
  state.history = [row, ...state.history];
  state.recording = "idle";
  void emit("voice-done", { polished, recording_id: row.id });
  void emit("app-state", snapshot());
  return snapshot();
}

/** Streams progress on the same event the Rust download reports on. */
async function downloadModel() {
  state.downloadCancelled = false;
  const total = 147_951_465;
  for (let step = 1; step <= 20; step += 1) {
    if (state.downloadCancelled) {
      void emit("meeting-model-download", { name: "ggml-oriserve-hinglish-fp16.bin", received: 0, total, status: "cancelled", error: null });
      throw new Error("cancelled");
    }
    await sleep(150);
    void emit("meeting-model-download", { name: "ggml-oriserve-hinglish-fp16.bin", received: Math.round((total * step) / 20), total, status: "downloading", error: null });
  }
  void emit("meeting-model-download", { name: "ggml-oriserve-hinglish-fp16.bin", received: total, total, status: "verifying", error: null });
  await sleep(600);
  state.model = "current";
  void emit("meeting-model-download", { name: "ggml-oriserve-hinglish-fp16.bin", received: total, total, status: "done", error: null });
  return null;
}

type Args = Record<string, unknown> | undefined;

const commands: Record<string, (args: Args) => unknown> = {
  bootstrap: () => snapshot(),
  get_snapshot: () => snapshot(),
  toggle_recording: () => toggleRecording(),
  request_microphone: () => { state.granted = true; return snapshot(); },
  request_accessibility: () => { state.granted = true; return snapshot(); },
  request_input_monitoring: () => { state.granted = true; return snapshot(); },

  get_history: (args) => {
    const limit = Number(args?.limit ?? 50);
    const before = args?.before as number | null | undefined;
    return state.history.filter((row) => before == null || row.timestamp_ms < before).slice(0, limit);
  },
  delete_recording: (args) => { state.history = state.history.filter((row) => row.id !== args?.id); return null; },
  get_app_icon: (args) => fx.appIcon(String(args?.appKey ?? "")),
  get_app_identity: (args) => fx.appIdentity(String(args?.appKey ?? "")),
  get_favicon: () => null,
  get_app_usage: () => fx.appUsage(state.history),
  get_site_usage: () => (state.history.length ? fx.siteUsage : []),
  get_performance_snapshot: () => fx.performance(),

  list_dictionary: () => state.dictionary,
  add_dictionary_word: (args) => {
    const word = {
      id: Date.now(),
      written: String(args?.written ?? "New word"),
      heard: args?.heard ? String(args.heard) : null,
      source: "added" as const,
      created_at: Date.now(),
    };
    state.dictionary = [word, ...state.dictionary];
    return word;
  },
  delete_dictionary_word: (args) => { state.dictionary = state.dictionary.filter((w) => w.id !== args?.id); return null; },
  clear_dictionary: () => { state.dictionary = []; return null; },

  get_preferences: () => state.preferences,
  patch_preferences: (args) => { Object.assign(state.preferences, args?.update as object); return state.preferences; },
  get_desktop_prefs: () => state.desktopPrefs,
  set_desktop_prefs: (args) => { Object.assign(state.desktopPrefs, args?.prefs as object); return null; },

  get_stt_setup_policy: () => fx.sttPolicy,
  get_stt_runtime: () => ({ ...fx.sttRuntime, dictation_ready: state.model !== "none" }),
  local_model_inventory: () => fx.modelInventory(state.model),
  choose_installed_local_model: () => fx.modelInventory(state.model),
  remove_unused_local_dictation_models: () => ({ removed: [], freed_bytes: 0 }),
  delete_all_local_speech_models: () => { state.model = "none"; return { removed: [], freed_bytes: 0 }; },
  dictation_model_status: () => ({ installed: state.model === "current", size_bytes: 147_951_465, path: "~/Library/Application Support/AirNote/models" }),
  download_dictation_model: () => downloadModel(),
  meeting_cancel_model_download: () => { state.downloadCancelled = true; return null; },

  get_backend_endpoint: () => ({ url: "http://backend.airnote.mock", secret: "mock" }),
  get_enterprise_status: () => ({ connected: scenario === "ready" || scenario === "new" || scenario === "update", email: fx.MOCK_EMAIL, server_url: MOCK_SERVER, org_name: fx.MOCK_ORG, token: "mock-jwt" }),
  get_cloud_status: () => ({ connected: true, license_tier: "team", email: fx.MOCK_EMAIL }),
  list_workspaces: () => ({ orgs: [{ id: "org-emiac", name: fx.MOCK_ORG, slug: "emiac", role: "owner", is_active: true }], active_org_id: "org-emiac", personal_mode: false }),
  get_device_id: () => "mock-device",
  get_hostname: () => "Abhisheks-MacBook-Air",
  start_enterprise_oauth_listener: () => 0,
  openai_status: () => ({ connected: false, expires_at: null, connected_at: null }),
  browser_automation_status: () => [
    { app_key: "com.google.Chrome", name: "Google Chrome", running: true, status: "granted" },
    { app_key: "com.apple.Safari", name: "Safari", running: false, status: "unknown" },
  ],
  developer_get_settings: () => ({ settings: { enabled: false, command_key: "", profiles: [] }, warnings: [] }),
  developer_save_settings: (args) => ({ settings: args?.settings ?? { enabled: false, command_key: "", profiles: [] }, warnings: [] }),
  get_debug_logs: () => ({ desktop_path: "~/Library/Logs/AirNote/desktop.log", backend_path: "~/Library/Logs/AirNote/backend.log", desktop: "(mock)", backend: "(mock)", combined: "(mock)", truncated: false }),
  read_backend_log: () => "[mock] backend log is only available in the desktop app",
  backend_log_location: () => "~/Library/Logs/AirNote/backend.log",
  get_status_bar_position: () => ({ x: 0, y: 0, placement: "bottom-center" }),
  screen_recording_granted: () => true,
  send_invite_email: () => ({ sent: true }),

  // Tauri plugins the app calls through @tauri-apps/api.
  "plugin:app|version": () => "2.5.0",
  "plugin:app|name": () => "AirNote",
  "plugin:app|tauri_version": () => "2.10.1",
  "plugin:updater|check": () => null,
  "plugin:notification|is_permission_granted": () => true,
  "plugin:notification|request_permission": () => "granted",
};

const warned = new Set<string>();
mockWindows("main");
mockIPC(
  (cmd, args) => {
    const handler = commands[cmd];
    if (handler) return handler(args as Args);
    // Anything unlisted is a side effect (open a settings pane, move a window):
    // succeed quietly, and say so once in the console.
    if (!warned.has(cmd)) {
      warned.add(cmd);
      console.debug(`[mock] ${cmd} → null (no mock handler)`);
    }
    return null;
  },
  { shouldMockEvents: true },
);

// ── HTTP ─────────────────────────────────────────────────────────────────────
// Every cross-origin request is answered here, so the preview never reaches a
// real server — not the local backend, not the control plane.

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

const http: [RegExp, (url: URL, init?: RequestInit) => Response][] = [
  [/\/v1\/health/, () => json({ ok: true })],
  [/\/v1\/auth\/me$/, () => json({ account: { id: "mock-account", email: fx.MOCK_EMAIL } })],
  [/\/v1\/auth\/desktop-email$/, (_url, init) => {
    const body = JSON.parse(String(init?.body ?? "{}")) as { email?: string };
    return json({ token: "mock-jwt", account: { id: "mock-account", email: body.email || fx.MOCK_EMAIL } });
  }],
  [/\/v1\/orgs\/me$/, () => json({ org: { id: "org-emiac", name: fx.MOCK_ORG } })],
  [/\/v1\/license\/check$/, () => json({ active: true })],
  [/\/v1\/openai\/status$/, () => json({ connected: false })],
  [/\/v1\/server-migration\/status$/, () => json({ status: "completed", migration_version: 1, uploaded_history_count: 0, uploaded_vocab_count: 0, uploaded_alias_count: 0, uploaded_email_count: 0, uploaded_credentials_count: 0, signed_in: true })],
];

const realFetch = window.fetch.bind(window);
const passthrough = [location.origin, "https://fonts.googleapis.com", "https://fonts.gstatic.com"];
window.fetch = async (input, init) => {
  const url = new URL(input instanceof Request ? input.url : String(input), location.href);
  if (passthrough.includes(url.origin)) return realFetch(input, init);
  await sleep(80);
  const route = http.find(([pattern]) => pattern.test(url.pathname));
  return route ? route[1](url, init) : json({});
};

// ── Switcher ─────────────────────────────────────────────────────────────────
// Lives in a shadow root so the app's CSS can't restyle it and it can't restyle
// the app. Hidden inside the floating status-bar window.

function mountSwitcher() {
  const params = new URLSearchParams(location.search);
  if (params.get("view") === "statusbar" || location.hash === "#statusbar") return;
  const host = document.createElement("div");
  host.style.cssText = "position:fixed;right:12px;bottom:12px;z-index:2147483647";
  const root = host.attachShadow({ mode: "open" });
  const theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";
  const options = Object.entries(SCENARIOS)
    .map(([key, label]) => `<option value="${key}" ${key === scenario ? "selected" : ""}>${label}</option>`)
    .join("");
  root.innerHTML = `
    <style>
      .bar { display:flex; gap:6px; align-items:center; padding:5px 6px 5px 10px; border-radius:999px;
        font:500 11px/1 -apple-system, system-ui, sans-serif; color:#fff; background:rgba(17,17,17,.86);
        box-shadow:0 6px 24px rgba(0,0,0,.25); backdrop-filter:blur(8px); opacity:.55; transition:opacity .15s }
      .bar:hover { opacity:1 }
      b { letter-spacing:.08em; font-size:10px; color:#8fb8ff }
      select, button { font:inherit; color:#fff; background:rgba(255,255,255,.12); border:0; border-radius:999px; padding:5px 9px; cursor:pointer }
    </style>
    <div class="bar">
      <b>MOCK</b>
      <select aria-label="Scenario">${options}</select>
      <button data-theme>${theme === "light" ? "Dark" : "Light"}</button>
      <button data-reset title="Reset this scenario to its starting state">Reset</button>
    </div>`;
  const go = (next: Record<string, string>) => {
    const url = new URL(location.href);
    for (const [key, value] of Object.entries(next)) url.searchParams.set(key, value);
    location.href = url.toString();
  };
  root.querySelector("select")!.addEventListener("change", (event) => go({ scenario: (event.target as HTMLSelectElement).value }));
  root.querySelector("[data-theme]")!.addEventListener("click", () => go({ theme: theme === "light" ? "dark" : "light" }));
  root.querySelector("[data-reset]")!.addEventListener("click", () => {
    localStorage.removeItem(SCENARIO_KEY);
    go({ scenario });
  });
  document.body.appendChild(host);
}

if (document.body) mountSwitcher();
else document.addEventListener("DOMContentLoaded", mountSwitcher);

console.info(`[mock] AirNote browser preview · scenario "${scenario}" · ${SCENARIOS[scenario]}`);
