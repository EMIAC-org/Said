import { useEffect, useRef, useState } from "react";
import { X, Send, Check, Loader2, Heart, AlertCircle } from "lucide-react";
import { sendInviteEmail, openExternal } from "@/lib/invoke";

/* ════════════════════════════════════════════════════════════════════════════
   InviteTeamModal — single-input "invite a friend".
   AutoNote is free while we're early — no team/billing pitch.

   How sending works:
   - Tries the backend first (POST /v1/invite → Resend).
   - If the backend has no RESEND_API_KEY configured, it returns
     "fallback_mailto" and we open the user's mail client with a
     pre-written note. Either way the user gets a "sent" outcome.
   - On a real network/server error, we surface a small inline message
     and let the user retry.

   Design tokens used (match the rest of the app):
   - .input + .btn-primary utility classes
   - --primary (mint) for the hero accent — same as everywhere else
   - --surface-2 modal mat, --surface-3 input rest, --surface-4 borders
   ════════════════════════════════════════════════════════════════════════════ */

interface Props {
  open:   boolean;
  onClose: () => void;
}

const VALID_EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

const MAIL_SUBJECT = "You should try AutoNote";
const MAIL_BODY =
  "Hey — I've been using AutoNote to dictate and polish text. " +
  "It's quietly become my favourite way to write.\n\n" +
  "Thought you'd like it: https://autonote.app";

type SendState =
  | { kind: "idle" }
  | { kind: "sending" }
  | { kind: "sent" }
  | { kind: "error"; message: string };

export function InviteTeamModal({ open, onClose }: Props) {
  const [email, setEmail]    = useState("");
  const [state, setState]    = useState<SendState>({ kind: "idle" });
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  useEffect(() => {
    if (open) {
      setEmail("");
      setState({ kind: "idle" });
      setTimeout(() => inputRef.current?.focus(), 60);
    }
  }, [open]);

  if (!open) return null;

  const trimmed    = email.trim();
  const looksValid = trimmed === "" || VALID_EMAIL.test(trimmed);
  const canSend    = trimmed !== "" && VALID_EMAIL.test(trimmed) && state.kind !== "sending";

  async function handleSend() {
    if (!canSend) return;
    setState({ kind: "sending" });

    try {
      const result = await sendInviteEmail(trimmed);

      if (result.status === "fallback_mailto") {
        // Backend has no provider — open the user's mail app with the note pre-written.
        // Tauri's webview silently blocks window.open for mailto:, so route through
        // the native opener.
        const subject = encodeURIComponent(MAIL_SUBJECT);
        const body    = encodeURIComponent(MAIL_BODY);
        await openExternal(`mailto:${trimmed}?subject=${subject}&body=${body}`);
      }

      setState({ kind: "sent" });
      setTimeout(() => onClose(), 1100);
    } catch (err) {
      setState({
        kind: "error",
        message: "Couldn't send right now. Try again in a moment.",
      });
    }
  }

  const submitting = state.kind === "sending";
  const submitted  = state.kind === "sent";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{
        background: "hsl(220 50% 2% / 0.55)",
        backdropFilter: "blur(8px)",
        WebkitBackdropFilter: "blur(8px)",
        animation: "fadeIn 0.18s ease-out",
      }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="rounded-[20px] overflow-hidden flex flex-col relative"
        style={{
          background: "hsl(var(--surface-2))",
          width:  "min(440px, 92vw)",
          boxShadow:
            "inset 0 1px 0 hsl(0 0% 100% / 0.06), 0 30px 80px hsl(220 60% 2% / 0.65)",
        }}
      >
        {/* Mint wash top-right — same hero glow used across the app */}
        <div
          aria-hidden
          className="absolute pointer-events-none"
          style={{
            right: -100, top: -120, width: 320, height: 320, borderRadius: "50%",
            background: "radial-gradient(circle, hsl(var(--primary) / 0.12) 0%, transparent 70%)",
          }}
        />

        {/* Close — floats top-right (same icon-button treatment as Topbar) */}
        <button
          onClick={onClose}
          aria-label="Close"
          className="absolute top-3.5 right-3.5 z-10 w-8 h-8 rounded-full flex items-center justify-center transition-colors"
          style={{ color: "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = "hsl(var(--surface-4))";
            e.currentTarget.style.color      = "hsl(var(--foreground))";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "transparent";
            e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
          }}
        >
          <X size={14} />
        </button>

        <div className="relative px-9 pt-10 pb-8 flex flex-col items-center">

          {/* Heart icon — mint chip, matches the chip-mint pattern used elsewhere */}
          <div
            className="w-12 h-12 rounded-2xl flex items-center justify-center mb-5"
            style={{
              background: "hsl(var(--primary) / 0.14)",
              color:      "hsl(var(--primary))",
              boxShadow:  "inset 0 0 0 1px hsl(var(--primary) / 0.22), 0 6px 18px hsl(var(--primary) / 0.18)",
            }}
          >
            <Heart size={20} strokeWidth={2.2} fill="currentColor" fillOpacity={0.18} />
          </div>

          {/* Headline — same scale & tracking as other modal titles (Topbar/Settings) */}
          <h2
            className="text-[22px] font-extrabold tracking-tight text-center mb-2"
            style={{
              color: "hsl(var(--foreground))",
              letterSpacing: "-0.02em",
            }}
          >
            Invite a friend
          </h2>

          {/* Sub-line — muted, same scale as DashboardCards body copy */}
          <p
            className="text-[13px] text-center max-w-[320px] leading-relaxed mb-7"
            style={{ color: "hsl(var(--muted-foreground))" }}
          >
            AutoNote is free while we're early. If someone in your life would love it, send them a note.
          </p>

          {/* Email input — uses .input utility for full token consistency */}
          <input
            ref={inputRef}
            type="email"
            value={email}
            onChange={(e) => {
              setEmail(e.target.value);
              if (state.kind === "error") setState({ kind: "idle" });
            }}
            onKeyDown={(e) => { if (e.key === "Enter") handleSend(); }}
            placeholder="friend@example.com"
            autoComplete="email"
            disabled={submitting || submitted}
            className="input"
            style={{
              fontSize: 14,
              padding: "12px 14px",
              borderRadius: 12,
              ...(looksValid ? {} : {
                borderColor: "hsl(354 78% 60% / 0.55)",
                boxShadow:   "0 0 0 3px hsl(354 78% 60% / 0.10)",
              }),
            }}
          />

          {/* Inline error — only when send fails */}
          {state.kind === "error" && (
            <div
              className="w-full mt-3 flex items-center gap-2 px-3 py-2 rounded-lg"
              style={{
                background: "hsl(354 78% 60% / 0.10)",
                color:      "hsl(354 78% 75%)",
                boxShadow:  "inset 0 0 0 1px hsl(354 78% 60% / 0.25)",
              }}
            >
              <AlertCircle size={13} className="flex-shrink-0" />
              <span className="text-[12px] font-medium">{state.message}</span>
            </div>
          )}

          {/* Send — mint .btn-primary pill, full width. Same glow + hover
              treatment used by every other primary CTA in the app. */}
          <button
            onClick={handleSend}
            disabled={!canSend || submitting || submitted}
            className="btn-primary mt-4 w-full justify-center py-3 rounded-xl"
            style={{ fontSize: 13.5 }}
          >
            {submitting ? (
              <>
                <Loader2 size={14} className="animate-spin" />
                Sending…
              </>
            ) : submitted ? (
              <>
                <Check size={14} strokeWidth={2.6} />
                Sent
              </>
            ) : (
              <>
                <Send size={13} />
                Send invite
              </>
            )}
          </button>

          {/* Footnote — tiny muted hint, one line */}
          <p
            className="text-[11px] text-center mt-5"
            style={{ color: "hsl(var(--muted-foreground))", opacity: 0.7 }}
          >
            A short note from AutoNote. No account needed.
          </p>
        </div>
      </div>
    </div>
  );
}
