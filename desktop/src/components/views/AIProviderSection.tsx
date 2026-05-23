import { useCallback, useEffect, useState } from "react";
import {
  Loader2, Check, ExternalLink, LogOut, Sparkles, ChevronDown,
} from "lucide-react";
import {
  getOpenAIStatus,
  initiateOpenAIConnect,
  completeOpenAIConnect,
  disconnectOpenAI,
  isConnected as isEnterpriseConnected,
  type OpenAIStatus,
} from "@/lib/enterprise";

// ── Plan type options ────────────────────────────────────────────────────────

const PLAN_OPTIONS = [
  { value: "plus", label: "Plus ($20/mo)" },
  { value: "pro",  label: "Pro ($100/mo)" },
] as const;

type PlanType = (typeof PLAN_OPTIONS)[number]["value"];

// ── State machine ────────────────────────────────────────────────────────────

type ViewState = "loading" | "disconnected" | "connecting" | "connected";

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Extract the `code` query param from a pasted callback URL */
function extractCodeFromUrl(raw: string): string | null {
  try {
    const url = new URL(raw.trim());
    return url.searchParams.get("code");
  } catch {
    return null;
  }
}

function formatDate(iso?: string): string {
  if (!iso) return "";
  return new Date(iso).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ── Component ────────────────────────────────────────────────────────────────

export function AIProviderSection() {
  const [view, setView] = useState<ViewState>("loading");
  const [status, setStatus] = useState<OpenAIStatus | null>(null);

  // Connecting flow state
  const [authUrl, setAuthUrl] = useState("");
  const [codeVerifier, setCodeVerifier] = useState("");
  const [callbackUrl, setCallbackUrl] = useState("");
  const [planType, setPlanType] = useState<PlanType>("plus");
  const [label, setLabel] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);

  // ── Load status on mount ───────────────────────────────────────────────────

  const refresh = useCallback(async () => {
    if (!isEnterpriseConnected()) {
      setView("disconnected");
      return;
    }
    const s = await getOpenAIStatus();
    if (s?.connected) {
      setStatus(s);
      setView("connected");
    } else {
      setView("disconnected");
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // ── Initiate connect ───────────────────────────────────────────────────────

  async function handleConnect() {
    setError("");
    setSubmitting(true);
    const result = await initiateOpenAIConnect();
    setSubmitting(false);
    if (!result) {
      setError("Failed to start OpenAI connection. Check your enterprise server.");
      return;
    }
    setAuthUrl(result.auth_url);
    setCodeVerifier(result.code_verifier);
    setView("connecting");
  }

  // ── Complete connect ───────────────────────────────────────────────────────

  async function handleComplete() {
    setError("");
    const code = extractCodeFromUrl(callbackUrl);
    if (!code) {
      setError("Could not extract authorization code from the URL. Make sure you pasted the full callback URL.");
      return;
    }
    setSubmitting(true);
    const ok = await completeOpenAIConnect(code, codeVerifier, planType, label || undefined);
    setSubmitting(false);
    if (!ok) {
      setError("Failed to complete connection. The authorization code may have expired.");
      return;
    }
    // Reset connecting state and reload
    setCallbackUrl("");
    setLabel("");
    setAuthUrl("");
    setCodeVerifier("");
    await refresh();
  }

  // ── Disconnect ─────────────────────────────────────────────────────────────

  async function handleDisconnect() {
    if (!confirmDisconnect) {
      setConfirmDisconnect(true);
      return;
    }
    setSubmitting(true);
    await disconnectOpenAI();
    setSubmitting(false);
    setConfirmDisconnect(false);
    setStatus(null);
    setView("disconnected");
  }

  // ── Loading ────────────────────────────────────────────────────────────────

  if (view === "loading") {
    return (
      <div className="mb-7">
        <SectionHeader />
        <div className="panel p-5 flex items-center justify-center" style={{ minHeight: 80 }}>
          <Loader2 size={18} className="animate-spin" style={{ color: "hsl(var(--muted-foreground))" }} />
        </div>
      </div>
    );
  }

  // ── Connected ──────────────────────────────────────────────────────────────

  if (view === "connected" && status) {
    return (
      <div className="mb-7">
        <SectionHeader />
        <div className="panel overflow-hidden">
          {/* Status row */}
          <div className="flex items-center gap-4 px-5 py-4">
            <div
              className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
              style={{
                background: "hsl(145 60% 12%)",
                color: "hsl(145 70% 65%)",
              }}
            >
              <Check size={16} />
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <p className="text-[13px] font-medium text-foreground">
                  OpenAI Connected
                </p>
                {status.plan_type && (
                  <span
                    className="text-[10px] font-semibold px-2 py-0.5 rounded-full uppercase"
                    style={{
                      background: "hsl(226 80% 78% / 0.14)",
                      color: "hsl(226 80% 84%)",
                    }}
                  >
                    {status.plan_type}
                  </span>
                )}
              </div>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                {status.label && <span>{status.label} &middot; </span>}
                {status.connected_at && <span>Connected {formatDate(status.connected_at)}</span>}
              </p>
            </div>
          </div>

          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

          {/* Disconnect row */}
          <div className="flex items-center gap-4 px-5 py-4">
            <div
              className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
              style={{
                background: "hsl(var(--surface-4))",
                color: "hsl(var(--muted-foreground))",
              }}
            >
              <Sparkles size={16} />
            </div>
            <div className="flex-1 min-w-0">
              <p className="text-[13px] font-medium text-foreground">AI Features</p>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                Meeting summaries, action items, and decisions are active.
              </p>
            </div>
            <div className="flex-shrink-0 ml-4">
              {confirmDisconnect ? (
                <div className="flex items-center gap-2">
                  <span className="text-[11px] text-muted-foreground">Are you sure?</span>
                  <button
                    onClick={handleDisconnect}
                    disabled={submitting}
                    className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1.5 transition-colors"
                    style={{
                      background: "hsl(0 60% 16%)",
                      color: "hsl(0 75% 72%)",
                    }}
                  >
                    {submitting ? <Loader2 size={11} className="animate-spin" /> : <LogOut size={11} />}
                    Yes, disconnect
                  </button>
                  <button
                    onClick={() => setConfirmDisconnect(false)}
                    className="text-[12px] font-medium px-3 py-1.5 rounded-lg transition-colors"
                    style={{
                      background: "hsl(var(--surface-4))",
                      color: "hsl(var(--foreground))",
                    }}
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <button
                  onClick={handleDisconnect}
                  className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1.5 transition-colors"
                  style={{
                    background: "hsl(0 60% 16%)",
                    color: "hsl(0 75% 72%)",
                  }}
                >
                  <LogOut size={11} />
                  Disconnect
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    );
  }

  // ── Connecting (OAuth flow) ────────────────────────────────────────────────

  if (view === "connecting") {
    return (
      <div className="mb-7">
        <SectionHeader />
        <div className="panel p-5 space-y-4">
          {/* Step 1: open auth URL */}
          <div>
            <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
              <span
                className="w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-bold"
                style={{ background: "hsl(var(--primary) / 0.14)", color: "hsl(var(--primary))" }}
              >
                1
              </span>
              Sign in to OpenAI
            </p>
            <p className="text-[11.5px] text-muted-foreground mb-2 leading-relaxed pl-7">
              Click the button below to open OpenAI's authorization page. Sign in and authorize AirNote.
            </p>
            <div className="pl-7">
              <button
                onClick={() => window.open(authUrl, "_blank")}
                className="inline-flex items-center gap-2 py-2 px-4 rounded-xl text-[12.5px] font-semibold transition-all"
                style={{
                  background: "hsl(var(--primary) / 0.14)",
                  color: "hsl(var(--foreground))",
                  boxShadow: "inset 0 0 0 1px hsl(var(--primary) / 0.4)",
                }}
              >
                <ExternalLink size={13} />
                Open OpenAI Authorization
              </button>
            </div>
          </div>

          {/* Step 2: paste callback URL */}
          <div>
            <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
              <span
                className="w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-bold"
                style={{ background: "hsl(var(--primary) / 0.14)", color: "hsl(var(--primary))" }}
              >
                2
              </span>
              Paste callback URL
            </p>
            <p className="text-[11.5px] text-muted-foreground mb-2 leading-relaxed pl-7">
              After authorizing, you'll be redirected. Paste the full URL from the browser's address bar.
            </p>
            <div className="pl-7">
              <input
                type="url"
                placeholder="https://...?code=abc123&state=..."
                value={callbackUrl}
                onChange={(e) => {
                  setCallbackUrl(e.target.value);
                  setError("");
                }}
                className="input text-[12px]"
              />
            </div>
          </div>

          {/* Plan type + label */}
          <div className="grid gap-4 pl-7" style={{ gridTemplateColumns: "1fr 1fr" }}>
            <div>
              <p className="text-[12px] font-semibold text-foreground mb-1.5">Plan type</p>
              <div className="relative">
                <select
                  value={planType}
                  onChange={(e) => setPlanType(e.target.value as PlanType)}
                  className="input text-[12px] appearance-none pr-8 cursor-pointer"
                >
                  {PLAN_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>{opt.label}</option>
                  ))}
                </select>
                <ChevronDown
                  size={13}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 pointer-events-none"
                  style={{ color: "hsl(var(--muted-foreground))" }}
                />
              </div>
            </div>
            <div>
              <p className="text-[12px] font-semibold text-foreground mb-1.5">Label (optional)</p>
              <input
                type="text"
                placeholder="e.g. Team shared account"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                className="input text-[12px]"
              />
            </div>
          </div>

          {/* Error */}
          {error && (
            <div
              className="rounded-lg px-3 py-2 text-[12px]"
              style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
            >
              {error}
            </div>
          )}

          {/* Actions */}
          <div className="flex items-center gap-3 pl-7 pt-1">
            <button
              onClick={handleComplete}
              disabled={submitting || !callbackUrl.trim()}
              className="btn-primary !py-2 !px-5 !text-[12.5px] flex items-center gap-2"
            >
              {submitting ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <Check size={13} />
              )}
              Complete Connection
            </button>
            <button
              onClick={() => {
                setView("disconnected");
                setAuthUrl("");
                setCodeVerifier("");
                setCallbackUrl("");
                setError("");
              }}
              className="btn-ghost !py-2 !px-4 !text-[12px]"
            >
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ── Disconnected (default) ─────────────────────────────────────────────────

  return (
    <div className="mb-7">
      <SectionHeader />
      <div className="panel p-5 space-y-4">
        <div className="flex items-start gap-3">
          <div
            className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 mt-0.5"
            style={{
              background: "hsl(var(--surface-4))",
              color: "hsl(var(--accent-violet))",
            }}
          >
            <Sparkles size={16} />
          </div>
          <div>
            <p className="text-[13px] font-medium text-foreground">Connect an OpenAI account</p>
            <p className="text-[12px] text-muted-foreground mt-1 leading-relaxed" style={{ maxWidth: 420 }}>
              Link your organization's OpenAI account to enable AI-powered meeting summaries,
              task extraction, and decision tracking across all meetings.
            </p>
          </div>
        </div>

        {error && (
          <div
            className="rounded-lg px-3 py-2 text-[12px]"
            style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
          >
            {error}
          </div>
        )}

        <div>
          <button
            onClick={handleConnect}
            disabled={submitting || !isEnterpriseConnected()}
            className="inline-flex items-center gap-2 py-2.5 px-5 rounded-xl text-[13px] font-semibold transition-all"
            style={{
              background: "hsl(var(--primary) / 0.14)",
              color: "hsl(var(--foreground))",
              boxShadow: "inset 0 0 0 1px hsl(var(--primary) / 0.4)",
            }}
          >
            {submitting ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <ExternalLink size={14} />
            )}
            Connect OpenAI Account
          </button>
          {!isEnterpriseConnected() && (
            <p className="text-[11px] text-muted-foreground mt-2">
              Connect to an enterprise server first to enable AI provider integration.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Section header ───────────────────────────────────────────────────────────

function SectionHeader() {
  return (
    <p className="section-label px-1 mb-2.5 flex items-center gap-2">
      <span
        className="inline-block w-1 h-1 rounded-full"
        style={{ background: "hsl(var(--accent-violet))" }}
      />
      AI Provider
    </p>
  );
}
