// Post-update "required step" gate.
//
// Most users are already onboarded and installed — they get new features via the
// update pipeline and NEVER re-enter onboarding (that only shows for a fresh
// setup). So any newly-required step would be invisible to nearly everyone.
//
// This versioned flag fixes that: bump MIGRATION_VERSION whenever a new mandatory
// post-update step is added, and the app forces already-onboarded users through
// just the outstanding step(s) once, then stamps the version.
//
//   v1 → "Meet the new on-device model" page (install the new model, or keep the
//        current setup). Everyone sees it once after updating.
//   v2 → adds the hotkey step (pick any modifier / Caps Lock / Fn) after the model.
export const MIGRATION_VERSION = 2;

const STORAGE_KEY = "said:migration-done";

/** The migration version the user has already satisfied (0 if never). */
export function loadMigrationDone(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const n = raw ? parseInt(raw, 10) : 0;
    return Number.isFinite(n) && n > 0 ? n : 0;
  } catch {
    return 0;
  }
}

/** Mark all current required migration steps as satisfied. */
export function markMigrationDone(): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(MIGRATION_VERSION));
  } catch {
    /* ignore quota errors */
  }
}
