import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, ExternalLink, RotateCw } from "lucide-react";
import { classifyError, type ErrorActionKind, type FriendlyError } from "../lib/friendlyError";

interface Props {
  /** Raw error (string/Error) or a pre-classified FriendlyError. Null → renders nothing. */
  error: unknown;
  /** Called when the user taps a `retry`-kind action. Without it, retry actions are hidden. */
  onRetry?: () => void;
  /** Extra classes on the wrapper. */
  className?: string;
}

const SETTINGS_COMMAND: Partial<Record<ErrorActionKind, string>> = {
  mic: "open_microphone_settings",
  accessibility: "open_accessibility_settings",
  "input-monitoring": "open_input_monitoring_settings",
};

/**
 * Actionable error row: friendly copy + a one-click fix button when we can infer
 * one. Permission errors deep-link to the right macOS privacy pane (self-served);
 * transient errors surface a Retry that calls back into the caller's operation.
 */
export function ErrorNotice({ error, onRetry, className }: Props) {
  if (error == null || error === "") return null;
  const fe: FriendlyError =
    typeof error === "object" && error !== null && "message" in error && !(error instanceof Error)
      ? (error as FriendlyError)
      : classifyError(error);

  const action = fe.action;
  const isRetry = action?.kind === "retry";
  // Hide a retry action we can't wire; keep OS-settings actions always.
  const showAction = action && (!isRetry || !!onRetry);

  const runAction = () => {
    if (!action) return;
    if (action.kind === "retry") {
      onRetry?.();
      return;
    }
    const cmd = SETTINGS_COMMAND[action.kind];
    if (cmd) void invoke(cmd).catch(() => {});
  };

  return (
    <div className={`err-notice ${className ?? ""}`}>
      <AlertTriangle size={13} className="err-notice-icon" />
      <span className="err-notice-msg">{fe.message}</span>
      {showAction && action && (
        <button type="button" className="err-notice-action" onClick={runAction}>
          {isRetry ? <RotateCw size={11} /> : <ExternalLink size={11} />}
          {action.label}
        </button>
      )}
    </div>
  );
}
