import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, Check, Keyboard } from "lucide-react";
import {
  codeToHotkeyId,
  hotkeyDisplay,
  hotkeyMode,
  hotkeyOptions,
  hotkeyWarning,
  type Platform,
} from "@/lib/hotkeys";

interface Props {
  /** Current stored hotkey id (e.g. "caps_lock", "right_command"). */
  value: string;
  onChange: (id: string) => void;
  platform: Platform;
  disabled?: boolean;
}

/**
 * VoiceTypr-style hold-to-talk hotkey picker: press the key you want to capture
 * it (via DOM keydown, so no risky OS-tap capture mode), or tap a chip. Only
 * modifiers / Caps Lock / Fn are accepted — a bare typing key can't be a global
 * hold trigger. Fn can't be captured via DOM, so it's offered as a chip.
 */
export function HotkeyPicker({ value, onChange, platform, disabled }: Props) {
  const [capturing, setCapturing] = useState(false);
  const [hint, setHint] = useState("");
  const isWindows = platform === "windows";
  const options = hotkeyOptions(platform);
  const current = hotkeyDisplay(value, platform);
  const mode = hotkeyMode(current.id, platform);
  const warning = hotkeyWarning(current.id, platform);

  const stop = useCallback(() => {
    setCapturing(false);
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setHint("");
        stop();
        return;
      }
      const id = codeToHotkeyId(e.code);
      if (id) {
        setHint("");
        onChange(id);
        stop();
      } else {
        setHint("That key can’t be held to talk — pick a modifier, Caps Lock, or Fn.");
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, onChange, stop]);

  return (
    <div className="hk-picker">
      <button
        type="button"
        disabled={disabled}
        onClick={() => {
          setHint("");
          setCapturing((c) => !c);
        }}
        className={`hk-capture ${capturing ? "hk-capturing" : ""}`}
      >
        {capturing ? (
          <span className="hk-capture-prompt">
            <Keyboard size={14} /> Press a modifier key… <span className="hk-esc">Esc to cancel</span>
          </span>
        ) : (
          <span className="hk-current">
            <span className="hk-keycap">{current.glyph}</span>
            <span className="hk-current-label">{current.label}</span>
            <span className="hk-mode">{mode === "toggle" ? "tap to start/stop" : "hold to talk"}</span>
            <span className="hk-change">Change</span>
          </span>
        )}
      </button>

      {hint && <p className="hk-hint">{hint}</p>}
      {capturing && !hint && (
        <p className="hk-capture-note">
          Some keys (Fn / Globe, media keys) can’t be detected here — tap it below instead.
        </p>
      )}
      {!capturing && warning && (
        <p className={`hk-warn hk-warn-${warning.severity}`}>
          <AlertTriangle size={12} />
          <span>{warning.text}</span>
        </p>
      )}

      <div className="hk-chips">
        {options.map((opt) => {
          const active = opt.id === current.id;
          return (
            <button
              key={opt.id}
              type="button"
              disabled={disabled}
              onClick={() => {
                setHint("");
                setCapturing(false);
                onChange(opt.id);
              }}
              className={`hk-chip ${active ? "hk-chip-active" : ""}`}
              title={opt.label}
            >
              <span className="hk-chip-glyph">{opt.glyph}</span>
              <span className="hk-chip-label">{opt.label}</span>
              {active && <Check size={11} className="hk-chip-check" />}
            </button>
          );
        })}
      </div>

      <p className="hk-foot">
        {isWindows
          ? "A single key you hold while speaking."
          : "A single key you hold while speaking. macOS needs Input Monitoring for global hotkeys."}
      </p>
    </div>
  );
}
