import { useState } from "react";
import { Copy, Check } from "lucide-react";

/** Best-effort full-detail string from any thrown value — keeps the stack when
 *  present (Error), the raw text (Tauri commands reject with a String), or a
 *  pretty JSON dump, so nothing about the failure is lost. */
export function describeError(e: unknown): string {
  if (e == null) return "";
  if (typeof e === "string") return e;
  if (e instanceof Error) {
    const stack = e.stack?.trim();
    return stack && stack.length > e.message.length ? stack : `${e.name}: ${e.message}`;
  }
  try {
    return JSON.stringify(e, null, 2);
  } catch {
    return String(e);
  }
}

/** Inline, always-visible, COPYABLE error box. Shows the exact failure text
 *  (monospace, selectable) with a one-click Copy button so a user on another
 *  machine can paste the precise error back to us. Renders nothing when empty. */
export function CopyableError({
  title = "Something went wrong",
  detail,
}: {
  title?: string;
  detail: string;
}) {
  const [copied, setCopied] = useState(false);
  if (!detail?.trim()) return null;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(detail);
    } catch {
      // Fallback for webviews without the async clipboard API.
      const ta = document.createElement("textarea");
      ta.value = detail;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* ignore */
      }
      document.body.removeChild(ta);
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div
      className="rounded-xl p-3 text-left"
      style={{
        border: "1px solid hsl(var(--destructive) / 0.4)",
        background: "hsl(var(--destructive) / 0.08)",
      }}
    >
      <div className="flex items-center justify-between gap-2 mb-1.5">
        <span
          className="text-[12px] font-semibold"
          style={{ color: "hsl(var(--destructive))" }}
        >
          {title}
        </span>
        <button
          type="button"
          onClick={() => void copy()}
          className="flex items-center gap-1 text-[11px] px-2 py-1 rounded-md transition-colors hover:bg-white/5"
          style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))" }}
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre
        className="text-[11px] whitespace-pre-wrap break-words select-text font-mono m-0"
        style={{
          color: "hsl(var(--muted-foreground))",
          maxHeight: 180,
          overflow: "auto",
        }}
      >
        {detail}
      </pre>
    </div>
  );
}
