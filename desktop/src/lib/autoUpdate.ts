import { getVersion } from "@tauri-apps/api/app";
import { emit } from "@tauri-apps/api/event";
import { check } from "@tauri-apps/plugin-updater";

const LAST_CHECK_KEY = "airnote:auto-update:last-check-ms";
const READY_VERSION_KEY = "airnote:auto-update:ready-version";
const DAY_MS = 24 * 60 * 60 * 1000;

let running = false;

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

function removeKey(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // Ignore.
  }
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
  let readyVersion = "";
  try {
    readyVersion = localStorage.getItem(READY_VERSION_KEY) || "";
  } catch {
    readyVersion = "";
  }
  if (!readyVersion) return false;

  try {
    const current = await getVersion();
    if (current === readyVersion) {
      removeKey(READY_VERSION_KEY);
      return false;
    }
  } catch {
    // If version lookup fails, still show the reminder below.
  }

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
    writeString(LAST_CHECK_KEY, String(now));

    const update = await check();
    if (!update) return;

    const version = update.version;
    await update.downloadAndInstall();
    writeString(READY_VERSION_KEY, version);
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

  return () => {
    stopped = true;
    window.clearTimeout(firstTimer);
    window.clearInterval(interval);
    window.removeEventListener("focus", trigger);
    document.removeEventListener("visibilitychange", trigger);
    window.removeEventListener("online", trigger);
    window.removeEventListener("pageshow", trigger);
  };
}
