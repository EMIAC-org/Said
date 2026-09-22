import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { DesktopPrefs, LocalModelInfo, LocalModelInventory } from "../lib/invoke";
import type { Preferences } from "../types";
import { DictationSttSection } from "./DictationSttSection";

const api = vi.hoisted(() => ({
  getDesktopPrefs: vi.fn(), setDesktopPrefs: vi.fn(), getPreferences: vi.fn(), patchPreferences: vi.fn(),
  getLocalModelInventory: vi.fn(), getSttSetupPolicy: vi.fn(), chooseInstalledLocalModel: vi.fn(),
  getLocalAsrRuntimeStatus: vi.fn(), removeUnusedLocalDictationModels: vi.fn(), deleteAllLocalSpeechModels: vi.fn(),
  invoke: vi.fn(), startLocalModelDownload: vi.fn(), cancelLocalModelDownload: vi.fn(),
  handlers: null as null | { onDone?: (model: string) => void },
  events: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock("../lib/invoke", () => api);
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, handler) => {
  api.events.set(name, handler); return () => api.events.delete(name);
}) }));
vi.mock("../lib/localModels", () => ({
  useLocalModelDownloads: (handlers: typeof api.handlers) => { api.handlers = handlers; return {}; },
  startLocalModelDownload: api.startLocalModelDownload,
  cancelLocalModelDownload: api.cancelLocalModelDownload,
  isCancelledDownload: (message: string) => /cancelled/i.test(message),
}));

let desktop: DesktopPrefs;
let preferences: Preferences;
let s1Installed: boolean;
function inventory(): LocalModelInventory {
  const models = [
    ["parakeet-en-q8", "Parakeet Unified EN 0.6B (Q8)", true],
    ["nemotron-q4", "Nemotron Streaming 3.5 (Q4)", true],
    ["oriserve", "Oriserve Hinglish", false],
  ].map(([key, name, installed]) => ({
    key, name, installed, size_bytes: 500_000_000, size_hint: "~500 MB", recommended: key === "nemotron-q4",
    active_for_dictation: desktop.dictation_stt === "local" && desktop.local_stt_model === key,
    required_for_meetings: key === "oriserve", compatibility_candidate: true, selectable: true,
    safe_to_remove: false, architecture: "parakeet", languages: ["en"], streaming: false,
    quantization: "Q8", license: null,
  })) as LocalModelInfo[];
  return { setup_kind: "local_required", recommended_model: "nemotron-q4", selected_model: desktop.local_stt_model,
    recommended_installed: true, existing_compatible_model: desktop.local_stt_model, models, reclaimable_bytes: 0 };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const radio = (name: string) => screen.getByRole("radio", { name }) as HTMLInputElement;

beforeEach(() => {
  vi.clearAllMocks(); api.events.clear(); api.handlers = null; s1Installed = true;
  desktop = { sentry_disabled: true, update_channel: "stable", message_polish_mode: false, polish_enabled: true,
    launch_at_login: false, beta_mode: false, browser_context_enabled: false, dictation_stt: "local", local_stt_model: "parakeet-en-q8" };
  preferences = { user_id: "test", selected_model: "s1-mini-q4", tone_preset: "neutral", custom_prompt: null,
    language: "en", output_language: "english", auto_paste: true, edit_capture: false, polish_text_hotkey: "", record_hotkey: "capslock",
    learning_enabled: false, server_runtime_enabled: true, server_audio_runtime_enabled: false,
    gateway_api_key: null, gemini_api_key: null, groq_api_key: null, deepinfra_api_key: null, llm_provider: "gateway" };
  api.getDesktopPrefs.mockImplementation(async () => ({ ...desktop }));
  api.getPreferences.mockImplementation(async () => ({ ...preferences }));
  api.getLocalModelInventory.mockImplementation(async () => inventory());
  api.getSttSetupPolicy.mockResolvedValue({ platform: "macos", cpu_family: "apple_silicon", total_memory_bytes: 24e9,
    setup_kind: "local_required", local_model: "nemotron-q4", local_model_name: "Nemotron Streaming 3.5 (Q4)", local_model_size_hint: "~496 MB" });
  api.getLocalAsrRuntimeStatus.mockResolvedValue(null);
  api.setDesktopPrefs.mockImplementation(async (next) => { desktop = { ...next }; });
  api.patchPreferences.mockImplementation(async (next) => { preferences = { ...preferences, ...next }; return { ...preferences }; });
  api.chooseInstalledLocalModel.mockImplementation(async (model) => { desktop = { ...desktop, local_stt_model: model, dictation_stt: "local" }; return inventory(); });
  api.startLocalModelDownload.mockImplementation(async (model) => { if (model === "s1-mini-q4") s1Installed = true; });
  api.invoke.mockImplementation(async (command) => {
    if (command === "get_s1_mini_status") return { installed: s1Installed, size_bytes: 484219808 };
    if (command === "download_local_model") { s1Installed = true; return; }
  });
});
afterEach(cleanup);

async function openPage() {
  render(<DictationSttSection onPrefsUpdated={() => {}} />);
  await screen.findByRole("radio", { name: "Parakeet Unified EN 0.6B (Q8)" });
}

describe("Models settings", () => {
  it("keeps the last saved STT selected while saving and after a failed switch", async () => {
    await openPage();
    const stalePrefs = { ...desktop };
    const olderRefresh = deferred<DesktopPrefs>();
    api.getDesktopPrefs.mockReturnValueOnce(olderRefresh.promise);
    act(() => api.handlers?.onDone?.("oriserve"));
    const save = deferred<LocalModelInventory>();
    api.chooseInstalledLocalModel.mockReturnValueOnce(save.promise);
    fireEvent.click(radio("Nemotron Streaming 3.5 (Q4)"));
    expect(radio("Parakeet Unified EN 0.6B (Q8)").checked).toBe(true);
    expect(radio("Nemotron Streaming 3.5 (Q4)").disabled).toBe(true);
    desktop = { ...desktop, local_stt_model: "nemotron-q4" };
    await act(async () => { save.resolve(inventory()); });
    await waitFor(() => expect(radio("Nemotron Streaming 3.5 (Q4)").checked).toBe(true));
    await act(async () => { olderRefresh.resolve(stalePrefs); });
    expect(radio("Nemotron Streaming 3.5 (Q4)").checked).toBe(true);
    expect(within(screen.getByRole("radiogroup", { name: "Speech-to-text model" })).getAllByRole("radio", { checked: true })).toHaveLength(1);
    api.chooseInstalledLocalModel.mockRejectedValueOnce(new Error("Could not switch speech model"));
    fireEvent.click(radio("Parakeet Unified EN 0.6B (Q8)"));
    await waitFor(() => expect(radio("Parakeet Unified EN 0.6B (Q8)").disabled).toBe(false));
    expect(radio("Nemotron Streaming 3.5 (Q4)").checked).toBe(true);
    api.setDesktopPrefs.mockRejectedValueOnce(new Error("Could not save speech settings"));
    fireEvent.click(radio("Whisper Large V3 Turbo"));
    await waitFor(() => expect(radio("Whisper Large V3 Turbo").disabled).toBe(false));
    expect(radio("Whisper Large V3 Turbo").checked).toBe(false);
    expect(radio("Nemotron Streaming 3.5 (Q4)").checked).toBe(true);
  });

  it("keeps cleanup enablement separate and remembers exactly one cleanup selection when off", async () => {
    desktop.polish_enabled = false;
    await openPage();
    expect(radio("S1-mini by Superwhisper").checked).toBe(true);
    expect(radio("S1-mini by Superwhisper").disabled).toBe(true);
    fireEvent.click(screen.getByRole("switch", { name: "Enable text cleanup" }));
    await waitFor(() => expect(radio("DeepSeek V4 Flash").disabled).toBe(false));
    fireEvent.click(radio("DeepSeek V4 Flash"));
    await waitFor(() => expect(radio("DeepSeek V4 Flash").checked).toBe(true));
    expect(radio("S1-mini by Superwhisper").checked).toBe(false);
    expect(api.setDesktopPrefs).toHaveBeenCalledTimes(1);
    expect(api.chooseInstalledLocalModel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("switch", { name: "Enable text cleanup" }));
    await waitFor(() => expect(radio("DeepSeek V4 Flash").disabled).toBe(true));
    expect(radio("DeepSeek V4 Flash").checked).toBe(true);
  });

  it("downloads S1 without changing either selected model or enabling cleanup", async () => {
    s1Installed = false; preferences.selected_model = "deepseek-v4-flash";
    await openPage();
    const download = deferred<void>();
    api.startLocalModelDownload.mockReturnValueOnce(download.promise);
    api.cancelLocalModelDownload.mockImplementationOnce(async () => { download.reject(new Error("Download cancelled")); });
    fireEvent.click(screen.getByRole("button", { name: /Download.*S1-mini/i }));
    const cancel = await screen.findByRole("button", { name: "Cancel" });
    expect((cancel as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(cancel);
    await screen.findByRole("button", { name: /Download.*S1-mini/i });
    expect(radio("DeepSeek V4 Flash").checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /Download.*S1-mini/i }));
    await waitFor(() => expect(radio("S1-mini by Superwhisper").disabled).toBe(false));
    expect(radio("DeepSeek V4 Flash").checked).toBe(true);
    expect(radio("Parakeet Unified EN 0.6B (Q8)").checked).toBe(true);
    expect(api.patchPreferences).not.toHaveBeenCalled();
    expect(api.setDesktopPrefs).not.toHaveBeenCalled();
  });
});
