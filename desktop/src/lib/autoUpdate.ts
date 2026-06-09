import { getVersion } from "@tauri-apps/api/app";
import { emit, listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";

const LAST_CHECK_KEY = "airnote:auto-update:last-check-ms";
const READY_VERSION_KEY = "airnote:auto-update:ready-version";
const REMINDER_SNOOZE_UNTIL_KEY = "airnote:auto-update:reminder-snooze-until-ms";
const DAY_MS = 24 * 60 * 60 * 1000;

/** Event the status-bar window emits when the user clicks Restart. The main
 * window owns the downloaded `Update` handle, so the apply must happen here. */
export const APPLY_UPDATE_EVENT = "airnote://apply-update";
export const APPLY_UPDATE_FAILED_EVENT = "airnote://apply-update-failed";

let running = false;

// The verified, downloaded-but-not-yet-installed update. Held in the main
// window's module scope between the background download and the user clicking
// Restart. On macOS `download()` stages the bundle without swapping it; on
// Windows it fetches the NSIS installer without closing the app. The actual
// swap/relaunch happens in `applyPendingUpdate()`.
let pendingUpdate: Update | null = null;

/** Check for an update and, if present, download + verify it WITHOUT installing.
 * Stores the handle in `pendingUpdate`. Returns the version, or null if none. */
export async function downloadUpdate(
  onProgress?: (event: DownloadEvent) => void,
): Promise<string | null> {
  const update = await check();
  if (!update) return null;
  await update.download(onProgress);
  pendingUpdate = update;
  writeString(READY_VERSION_KEY, update.version);
  return update.version;
}

/** Apply the previously downloaded update and relaunch. Idempotent-ish: if the
 * handle was lost (e.g. the webview reloaded), re-check and download first.
 * On Windows `install()` runs the NSIS installer, which closes the app and
 * relaunches it, so the trailing `relaunch()` only runs on macOS. */
export async function applyPendingUpdate(): Promise<void> {
  let update = pendingUpdate;
  if (!update) {
    update = await check();
    if (update) await update.download();
  }
  if (!update) {
    clearStoredReadyUpdateVersion();
    throw new Error("No downloaded update is available. Please check for updates and download it again.");
  }
  await update.install();
  pendingUpdate = null;
  clearStoredReadyUpdateVersion();
  await relaunch();
}

/** Ask the main app webview to apply the pending update.
 *
 * The downloaded `Update` handle belongs to the webview that performed the
 * download. Secondary windows, especially the status-bar HUD, cannot reliably
 * hold that handle. Emitting one shared event keeps every Restart/Relaunch
 * button on the same code path.
 */
export async function requestApplyPendingUpdate(): Promise<void> {
  await emit(APPLY_UPDATE_EVENT, { requestedAt: Date.now() });
}

function readNumber(key: string): number {
  try {
    const raw = localStorage.getItem(key);
    const parsed = raw ? Number(raw) : 0;
    return Number.isFinite(parsed) ? parsed : 0;
  } catch {
    return 0;
  }
}

function writeString(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Ignore storage failures; update checks should remain non-fatal.
  }
}

function writeNumber(key: string, value: number): void {
  writeString(key, String(value));
}

function removeKey(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // Ignore.
  }
}

export function getStoredReadyUpdateVersion(): string {
  try {
    return localStorage.getItem(READY_VERSION_KEY) || "";
  } catch {
    return "";
  }
}

export function clearStoredReadyUpdateVersion(): void {
  removeKey(READY_VERSION_KEY);
  clearReadyUpdateReminderSnooze();
}

function isReadyUpdateReminderSnoozed(): boolean {
  return readNumber(REMINDER_SNOOZE_UNTIL_KEY) > Date.now();
}

/** Hide the restart reminder until the snooze expires. The downloaded update stays staged. */
export function snoozeReadyUpdateReminder(durationMs = DAY_MS): void {
  writeNumber(REMINDER_SNOOZE_UNTIL_KEY, Date.now() + durationMs);
}

export function clearReadyUpdateReminderSnooze(): void {
  removeKey(REMINDER_SNOOZE_UNTIL_KEY);
}

export async function getPendingReadyUpdateVersion(): Promise<string> {
  const readyVersion = getStoredReadyUpdateVersion();
  if (!readyVersion) return "";

  try {
    const current = await getVersion();
    if (current === readyVersion) {
      clearStoredReadyUpdateVersion();
      return "";
    }
  } catch {
    // Keep showing the restart prompt if version lookup fails. Losing the
    // prompt is worse than asking the user to retry a valid update.
  }

  return readyVersion;
}

/** Ready update that should surface a restart reminder right now. */
export async function getPendingReadyUpdateReminder(): Promise<string> {
  const readyVersion = await getPendingReadyUpdateVersion();
  if (!readyVersion || isReadyUpdateReminderSnoozed()) return "";
  return readyVersion;
}

async function notifyReady(version: string, reminder = false): Promise<void> {
  await emit("auto-update-ready", {
    version,
    message: reminder
      ? `Update ${version} is ready. Restart AirNote to use it.`
      : `Update ${version} downloaded. Restart AirNote to use it.`,
  });
}

async function reconcileReadyVersion(): Promise<boolean> {
  const readyVersion = await getPendingReadyUpdateVersion();
  if (!readyVersion) return false;

  if (isReadyUpdateReminderSnoozed()) return true;

  await notifyReady(readyVersion, true);
  return true;
}

async function runDailyCheck(): Promise<void> {
  if (running) return;
  running = true;
  try {
    const hasPendingReadyUpdate = await reconcileReadyVersion();
    if (hasPendingReadyUpdate) return;

    const now = Date.now();
    const last = readNumber(LAST_CHECK_KEY);
    if (last > 0 && now - last < DAY_MS) return;

    const version = await downloadUpdate();
    if (!version) {
      writeString(LAST_CHECK_KEY, String(now));
      return;
    }

    writeString(LAST_CHECK_KEY, String(now));
    await notifyReady(version);
  } catch (err) {
    console.warn("[auto-update] daily check failed:", err);
  } finally {
    running = false;
  }
}

export function startDailyAutoUpdateCheck(): () => void {
  let stopped = false;

  const trigger = () => {
    if (stopped) return;
    void runDailyCheck();
  };

  const firstTimer = window.setTimeout(() => {
    trigger();
  }, 10_000);
  const interval = window.setInterval(() => {
    trigger();
  }, 60 * 60 * 1000);

  // Macs are often slept instead of shut down. Timers pause during sleep, then
  // resume later; these foreground signals make the same guarded check run soon
  // after lid-open/app-return instead of waiting for the next hourly tick.
  window.addEventListener("focus", trigger);
  document.addEventListener("visibilitychange", trigger);
  window.addEventListener("online", trigger);
  window.addEventListener("pageshow", trigger);

  // The status-bar runs in a separate webview and cannot hold the downloaded
  // `Update` handle, so its Restart button emits this event for us to apply.
  const unlistenApply = listen(APPLY_UPDATE_EVENT, () => {
    void applyPendingUpdate().catch((err) => {
      const message = err instanceof Error ? err.message : String(err);
      console.warn("[auto-update] apply failed:", err);
      void emit(APPLY_UPDATE_FAILED_EVENT, { message });
    });
  });

  return () => {
    stopped = true;
    window.clearTimeout(firstTimer);
    window.clearInterval(interval);
    window.removeEventListener("focus", trigger);
    document.removeEventListener("visibilitychange", trigger);
    window.removeEventListener("online", trigger);
    window.removeEventListener("pageshow", trigger);
    void unlistenApply.then((fn) => fn());
  };
}
