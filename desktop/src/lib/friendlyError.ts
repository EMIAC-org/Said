// Turn raw backend/network error strings into calm, human copy. Used across the
// onboarding, model-download, and migration surfaces so a dropped connection
// reads like a gentle nudge instead of a stack-trace fragment.
//
// Two layers:
//   friendlyError(raw)   → just the copy (string). Back-compat, used everywhere.
//   classifyError(raw)   → { message, action? } so the UI can render a one-click
//                          fix (Retry / Re-download / Open Settings). See
//                          <ErrorNotice/>, which renders the action for you.

/** True when the browser/WebView reports no network connectivity. */
export function isOffline(): boolean {
  return typeof navigator !== "undefined" && navigator.onLine === false;
}

/**
 * The suggested one-click fix for an error.
 * - `retry`   → re-run the caller's operation (needs an onRetry handler).
 * - `mic` / `input-monitoring` / `accessibility` → open the matching macOS
 *   privacy pane (Windows opens the closest Settings page). Self-contained: the
 *   <ErrorNotice/> invokes the Tauri command itself, no handler needed.
 */
export type ErrorActionKind = "retry" | "mic" | "input-monitoring" | "accessibility";

export interface ErrorAction {
  label: string;
  kind: ErrorActionKind;
}

export interface FriendlyError {
  message: string;
  /** Suggested fix, if we could infer one. Absent → dead-end error, copy only. */
  action?: ErrorAction;
}

/**
 * Classify a raw error into friendly copy + an optional actionable fix.
 * Offline is checked first (most common, most actionable); then permission
 * denials (which need an OS Settings pane, not a retry); then the transient
 * network/timeout/integrity family (retry); finally a trimmed fallback.
 */
export function classifyError(raw: unknown, fallback = "Something went wrong. Please try again."): FriendlyError {
  const msg = raw instanceof Error ? raw.message : typeof raw === "string" ? raw : "";
  const e = msg.toLowerCase();

  if (isOffline()) {
    return {
      message: "You’re offline — connect to the internet and try again.",
      action: { label: "Try again", kind: "retry" },
    };
  }

  // Permission denials — a retry won't help; the user must grant access in
  // System Settings, so we deep-link to the exact pane.
  if (/microphone|mic access|no input device|permission.*(mic|audio)|audio.*permission/.test(e)) {
    return {
      message: "AirNote can’t hear your microphone — grant Microphone access to record.",
      action: { label: "Open Microphone settings", kind: "mic" },
    };
  }
  if (/accessibility|not trusted|axis|cgevent.*post|typing.*permission|paste.*permission/.test(e)) {
    return {
      message: "AirNote can’t type into apps yet — grant Accessibility access.",
      action: { label: "Open Accessibility settings", kind: "accessibility" },
    };
  }
  if (/input monitoring|cgeventtap|event tap|listen.*key|hotkey.*permission/.test(e)) {
    return {
      message: "AirNote can’t see the hotkey — grant Input Monitoring access.",
      action: { label: "Open Input Monitoring settings", kind: "input-monitoring" },
    };
  }

  // Transient failures — a retry is the right fix.
  if (/deepinfra.*(rate.?limit|429)|x-ratelimit/.test(e)) {
    return {
      message: "DeepInfra rate limit hit — wait a moment and try again.",
      action: { label: "Try again", kind: "retry" },
    };
  }
  if (/timed? ?out|timeout|deadline/.test(e)) {
    return {
      message: "That took too long — check your connection and try again.",
      action: { label: "Try again", kind: "retry" },
    };
  }
  if (
    /network|connection|connect|dns|resolve|unreachable|refused|reset|fetch|socket|tls|ssl|502|503|504|http 5\d\d/.test(
      e,
    )
  ) {
    return {
      message: "Couldn’t reach the server — check your internet and try again.",
      action: { label: "Try again", kind: "retry" },
    };
  }
  if (/sha-?256|integrity|corrupt|checksum|hash mismatch/.test(e)) {
    return {
      message: "The download was corrupted — re-download to fix it.",
      action: { label: "Re-download", kind: "retry" },
    };
  }

  return { message: msg.trim() || fallback };
}

/**
 * Map a raw error to friendly copy (string only). Thin wrapper over
 * {@link classifyError} kept for the many call sites that just render text.
 */
export function friendlyError(raw: unknown, fallback = "Something went wrong. Please try again."): string {
  return classifyError(raw, fallback).message;
}
