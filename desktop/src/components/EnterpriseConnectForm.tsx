import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Clock, ExternalLink, Link, Loader2, RotateCcw, X } from "lucide-react";
import { openExternal } from "@/lib/invoke";
import { LarkLogo } from "@/components/LarkLogo";
import {
  completeAuth,
  forgetWorkspaceUrl,
  getConnection,
  getPendingServerUrl,
  getRecentWorkspaceUrls,
  setPendingServerUrl,
  validateServer,
  type EnterpriseConnection,
} from "@/lib/enterprise";

type OAuthPhase = "idle" | "waiting" | "submitting" | "error";

export interface EnterpriseConnectFormProps {
  /** Called after OAuth completes successfully. */
  onConnected: (conn: EnterpriseConnection) => void;
  /** Pre-fill server URL (e.g. from a previous attempt). */
  initialServerUrl?: string;
  /** When true, hide the intro copy (Settings reuse). */
  compact?: boolean;
  /** Onboarding layout — full-width primary button. */
  variant?: "default" | "onboarding";
  /** Back out of OAuth waiting (e.g. onboarding goBack). */
  onCancel?: () => void;
  /** When set, the server URL is fixed (pulled from config/env): the URL input
   *  is hidden, the URL is auto-validated, and the form jumps straight to the
   *  Lark sign-in. Used by onboarding so users never type a server URL. */
  lockedServerUrl?: string;
  /** In locked mode, reveal a small development escape hatch for custom URLs. */
  allowCustomServerUrl?: boolean;
}

export function EnterpriseConnectForm({
  onConnected,
  initialServerUrl = "",
  compact = false,
  variant = "default",
  onCancel,
  lockedServerUrl,
  allowCustomServerUrl = false,
}: EnterpriseConnectFormProps) {
  const [serverUrl, setServerUrl] = useState(initialServerUrl);
  const [validating, setValidating] = useState(false);
  const [validated, setValidated] = useState(false);
  const [validationError, setValidationError] = useState("");
  const [oauthPhase, setOauthPhase] = useState<OAuthPhase>("idle");
  const [authUrl, setAuthUrl] = useState("");
  const [token, setToken] = useState("");
  const [tokenError, setTokenError] = useState("");
  const [showManualToken, setShowManualToken] = useState(false);
  const [recentUrls, setRecentUrls] = useState<string[]>(() => getRecentWorkspaceUrls());
  const [usingCustomServerUrl, setUsingCustomServerUrl] = useState(false);
  const serverUrlRef = useRef(serverUrl);
  serverUrlRef.current = serverUrl;
  const effectiveLockedServerUrl =
    lockedServerUrl && !(allowCustomServerUrl && usingCustomServerUrl)
      ? lockedServerUrl
      : undefined;

  const stopOAuthListener = useCallback(async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("stop_enterprise_oauth_listener");
    } catch {
      // ignore outside Tauri
    }
  }, []);

  const refreshRecents = useCallback(() => {
    setRecentUrls(getRecentWorkspaceUrls());
  }, []);

  const resetOAuth = useCallback(async () => {
    await stopOAuthListener();
    setOauthPhase("idle");
    setAuthUrl("");
    setToken("");
    setTokenError("");
    setShowManualToken(false);
  }, [stopOAuthListener]);

  const pickWorkspaceUrl = useCallback((url: string) => {
    setServerUrl(url);
    setValidated(false);
    setValidationError("");
    void resetOAuth();
  }, [resetOAuth]);

  const useCustomServerUrl = useCallback(() => {
    setUsingCustomServerUrl(true);
    setValidated(false);
    setValidationError("");
    const locked = lockedServerUrl?.trim().replace(/\/+$/, "");
    const recent = getRecentWorkspaceUrls().find((url) => url !== locked);
    setServerUrl(recent ?? "");
    void resetOAuth();
  }, [lockedServerUrl, resetOAuth]);

  const useDefaultServerUrl = useCallback(() => {
    if (!lockedServerUrl) return;
    setUsingCustomServerUrl(false);
    setValidationError("");
    setServerUrl(lockedServerUrl.trim().replace(/\/+$/, ""));
    void resetOAuth();
  }, [lockedServerUrl, resetOAuth]);

  useEffect(() => {
    const conn = getConnection();
    if (conn) {
      setServerUrl(conn.serverUrl);
      setValidated(true);
      return;
    }
    if (initialServerUrl) {
      setServerUrl(initialServerUrl);
      return;
    }
    const pending = getPendingServerUrl();
    if (pending) {
      setServerUrl(pending);
      return;
    }
    const recents = getRecentWorkspaceUrls();
    if (recents.length > 0) {
      setServerUrl(recents[0]);
    }
  }, [initialServerUrl]);

  // Locked server URL (from config/env): pin it, auto-validate, and skip the
  // URL-entry step so the user lands directly on the Lark sign-in.
  useEffect(() => {
    if (!effectiveLockedServerUrl) return;
    let alive = true;
    const url = effectiveLockedServerUrl.trim().replace(/\/+$/, "");
    setServerUrl(url);
    if (getConnection()) return; // already connected → Lark card already shows
    setValidating(true);
    setValidationError("");
    void (async () => {
      const ok = await validateServer(url).catch(() => false);
      if (!alive) return;
      setValidating(false);
      if (ok) setValidated(true);
      else setValidationError("Couldn't reach AirNote — check your connection and retry.");
    })();
    return () => {
      alive = false;
    };
  }, [effectiveLockedServerUrl]);

  const startOAuth = useCallback(async (url: string) => {
    const trimmed = url.trim().replace(/\/+$/, "");
    if (!trimmed) return;

    setPendingServerUrl(trimmed);
    setTokenError("");
    setShowManualToken(false);

    let callbackPort: number | null = null;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      callbackPort = await invoke<number>("start_enterprise_oauth_listener");
    } catch (err) {
      console.warn("[enterprise] localhost OAuth listener unavailable", err);
    }

    const params = new URLSearchParams();
    if (callbackPort != null) {
      params.set("callback_port", String(callbackPort));
    }
    const query = params.toString();
    const larkUrl = query ? `${trimmed}/auth/lark?${query}` : `${trimmed}/auth/lark`;

    setAuthUrl(larkUrl);
    const opened = await openExternal(larkUrl);
    if (!opened) {
      setOauthPhase("error");
      setTokenError("Couldn't open your browser. Try again or paste the token manually.");
      setShowManualToken(true);
      return;
    }
    setOauthPhase("waiting");
  }, []);

  async function handleValidate() {
    const trimmed = serverUrl.trim();
    if (!trimmed) return;
    setValidating(true);
    setValidationError("");
    setValidated(false);
    await resetOAuth();
    try {
      const ok = await validateServer(trimmed);
      if (ok) {
        setValidated(true);
        refreshRecents();
      } else {
        setValidationError("Server did not respond with a valid health check.");
      }
    } catch {
      setValidationError("Could not reach the server.");
    } finally {
      setValidating(false);
    }
  }

  const handleTokenSubmit = useCallback(async (manualToken?: string) => {
    const trimmed = (manualToken ?? token).trim();
    if (!trimmed) {
      setTokenError("Paste the token from the browser or click Open AirNote.");
      return;
    }
    setOauthPhase("submitting");
    setTokenError("");
    try {
      const conn = await completeAuth(serverUrlRef.current.trim(), trimmed);
      await resetOAuth();
      refreshRecents();
      onConnected(conn);
    } catch (e) {
      setOauthPhase("error");
      setTokenError((e as Error).message || "Invalid token");
    }
  }, [token, resetOAuth, refreshRecents, onConnected]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let alive = true;
    let timeoutId: ReturnType<typeof setTimeout> | undefined;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<{ token: string }>("enterprise-oauth-token", (ev) => {
        if (!alive || !ev.payload?.token) return;
        if (timeoutId) clearTimeout(timeoutId);
        setToken(ev.payload.token);
        void handleTokenSubmit(ev.payload.token);
      });
    })();

    return () => {
      alive = false;
      if (timeoutId) clearTimeout(timeoutId);
      unlisten?.();
      void (async () => {
        await stopOAuthListener();
      })();
    };
  }, [handleTokenSubmit, stopOAuthListener]);

  useEffect(() => {
    if (oauthPhase !== "waiting") return;
    const timeoutId = setTimeout(() => {
      setOauthPhase("error");
      setTokenError("Sign-in timed out after 5 minutes. Try again.");
      void (async () => {
        await stopOAuthListener();
      })();
    }, 5 * 60 * 1000);
    return () => clearTimeout(timeoutId);
  }, [oauthPhase, stopOAuthListener]);

  const isOnboarding = variant === "onboarding";
  const connectBtnClass = isOnboarding
    ? "btn-primary btn-lg w-full flex items-center justify-center gap-2"
    : "btn-primary !py-1.5 !px-4 !text-[12px] flex items-center gap-1.5 flex-shrink-0";
  const secondaryBtnClass =
    "px-3 py-1.5 rounded-lg text-[12px] font-semibold transition-colors border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30 flex items-center gap-1.5";

  const waitingForBrowser = oauthPhase === "waiting" || oauthPhase === "submitting";

  function RecentWorkspaces() {
    if (recentUrls.length === 0) return null;
    return (
      <div className="mt-3 space-y-2">
        <p className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-muted-foreground flex items-center gap-1.5">
          <Clock size={11} />
          Recent workspaces
        </p>
        <div className="flex flex-col gap-1.5">
          {recentUrls.map((url) => {
            const selected = serverUrl.trim().replace(/\/+$/, "") === url;
            return (
              <div
                key={url}
                className={`group flex items-center gap-2 rounded-lg border px-3 py-2 transition-colors ${
                  selected
                    ? "border-primary/40 bg-primary/10"
                    : "border-border hover:border-primary/25 hover:bg-white/[0.03]"
                }`}
              >
                <button
                  type="button"
                  onClick={() => pickWorkspaceUrl(url)}
                  disabled={waitingForBrowser}
                  className="flex-1 min-w-0 text-left text-[12px] font-mono truncate text-foreground/90 disabled:opacity-60"
                  title={url}
                >
                  {url}
                </button>
                {selected && (
                  <Check size={12} className="shrink-0 text-primary" aria-hidden />
                )}
                <button
                  type="button"
                  onClick={() => {
                    forgetWorkspaceUrl(url);
                    refreshRecents();
                  }}
                  disabled={waitingForBrowser}
                  className="shrink-0 p-1 rounded opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground transition-opacity disabled:opacity-0"
                  aria-label={`Remove ${url}`}
                >
                  <X size={12} />
                </button>
              </div>
            );
          })}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {!compact && (
        <p className="text-[12px] text-muted-foreground leading-relaxed">
          Connect to your organization&apos;s AirNote server, then sign in with your Lark account.
        </p>
      )}

      {effectiveLockedServerUrl && validating && !validated && (
        <div className="flex items-center gap-2 text-[12.5px] text-muted-foreground">
          <Loader2 size={14} className="animate-spin" />
          Connecting to AirNote…
        </div>
      )}

      {allowCustomServerUrl && lockedServerUrl && !usingCustomServerUrl && !waitingForBrowser && (
        <button
          type="button"
          onClick={useCustomServerUrl}
          className="w-full rounded-lg border px-3 py-2.5 transition-colors text-center"
          style={{ borderColor: "hsl(var(--border))", color: "hsl(var(--foreground))" }}
        >
          <span className="block text-[12px] font-medium" style={{ color: "hsl(var(--muted-foreground))" }}>
            Developing against another server?
          </span>
          <span className="block text-[12px] font-semibold mt-0.5" style={{ color: "hsl(var(--primary))" }}>
            Use custom URL →
          </span>
        </button>
      )}

      {!effectiveLockedServerUrl && (
      <div>
        <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
          <Link size={12} className="text-muted-foreground" />
          Server URL
        </p>
        {isOnboarding ? (
          <div className="flex flex-col gap-3">
            <input
              type="url"
              placeholder="http://localhost:3100"
              value={serverUrl}
              disabled={waitingForBrowser}
              onChange={(e) => {
                setServerUrl(e.target.value);
                setValidated(false);
                setValidationError("");
                void resetOAuth();
              }}
              className="input w-full text-[13px]"
            />
            {!validated && (
              <button
                onClick={() => void handleValidate()}
                disabled={validating || waitingForBrowser || !serverUrl.trim()}
                className={connectBtnClass}
              >
                {validating && <Loader2 size={14} className="animate-spin" />}
                {validating ? "Verifying…" : "Connect workspace"}
              </button>
            )}
            {!validated && <RecentWorkspaces />}
            {allowCustomServerUrl && lockedServerUrl && (
              <button
                type="button"
                onClick={useDefaultServerUrl}
                disabled={waitingForBrowser}
                className="text-[11px] text-muted-foreground hover:text-foreground transition-colors"
              >
                Use default AirNote server
              </button>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <input
                type="url"
                placeholder="http://localhost:3100"
                value={serverUrl}
                disabled={waitingForBrowser}
                onChange={(e) => {
                  setServerUrl(e.target.value);
                  setValidated(false);
                  setValidationError("");
                  void resetOAuth();
                }}
                className="input flex-1 text-[12px]"
              />
              {!validated && (
                <button
                  onClick={() => void handleValidate()}
                  disabled={validating || waitingForBrowser || !serverUrl.trim()}
                  className={connectBtnClass}
                >
                  {validating && <Loader2 size={12} className="animate-spin" />}
                  {validating ? "Verifying…" : "Connect"}
                </button>
              )}
            </div>
            {!validated && recentUrls.length > 0 && <RecentWorkspaces />}
          </div>
        )}
      </div>
      )}

      {validationError && (
        <div
          className="rounded-lg px-3 py-2 text-[12px]"
          style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
        >
          {validationError}
        </div>
      )}
      {effectiveLockedServerUrl && validationError && !validated && !validating && (
        <button onClick={() => void handleValidate()} className={connectBtnClass}>
          <RotateCcw size={14} />
          Retry
        </button>
      )}

      {validated && !waitingForBrowser && oauthPhase === "idle" && (
        <div className="rounded-2xl border border-border bg-white/[0.025] px-5 py-6 flex flex-col items-center text-center gap-4">
          <LarkLogo size={46} />
          <div className="space-y-1">
            <p className="text-[14px] font-semibold text-foreground">Sign in with Lark</p>
            <p className="text-[12px] text-muted-foreground leading-relaxed max-w-[260px]">
              Use your organization&apos;s Lark account. We&apos;ll open Lark in your browser and
              connect automatically.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void startOAuth(serverUrl)}
            className="w-full rounded-xl px-4 py-2.5 text-[13px] font-semibold flex items-center justify-center gap-2.5 transition-transform active:scale-[0.99]"
            style={{
              background: "#ffffff",
              color: "#1a1a1a",
              boxShadow: "0 6px 18px -8px rgba(0, 0, 0, 0.55)",
            }}
          >
            <LarkLogo size={18} />
            Continue with Lark
          </button>
        </div>
      )}

      {waitingForBrowser && (
        <div className="space-y-2.5 pt-1">
          {/* De-noised: one compact status strip — no wall of text, no Try-again
              (the inline "Reopen" already restarts OAuth). */}
          <div
            className="flex items-start gap-2.5 rounded-lg px-3 py-2.5"
            style={{ background: "hsl(210 50% 12% / 0.5)", boxShadow: "inset 0 0 0 1px hsl(210 60% 60% / 0.16)" }}
          >
            {oauthPhase === "submitting" ? (
              <Loader2 size={14} className="shrink-0 animate-spin mt-0.5" style={{ color: "hsl(210 80% 74%)" }} />
            ) : (
              <ExternalLink size={14} className="shrink-0 mt-0.5" style={{ color: "hsl(210 80% 74%)" }} />
            )}
            <div className="min-w-0">
              <p className="text-[12.5px] font-semibold" style={{ color: "hsl(210 75% 82%)" }}>
                {oauthPhase === "submitting" ? "Finishing sign-in…" : "Waiting for browser sign-in…"}
              </p>
              <p className="text-[11.5px] leading-snug mt-0.5" style={{ color: "hsl(210 25% 72% / 0.85)" }}>
                Finish in your browser — we&apos;ll connect automatically.
                {authUrl && oauthPhase === "waiting" && (
                  <>
                    {" "}
                    <button
                      type="button"
                      className="underline hover:opacity-100"
                      onClick={() => void openExternal(authUrl)}
                      title={authUrl}
                    >
                      Reopen
                    </button>
                  </>
                )}
              </p>
            </div>
          </div>

          {/* A single muted row replaces the old Cancel + Try-again + paste-token stack. */}
          <div className="flex items-center justify-between">
            {oauthPhase !== "submitting" ? (
              <button
                type="button"
                className="text-[11.5px] text-muted-foreground hover:text-foreground transition-colors"
                onClick={() => {
                  void resetOAuth();
                  onCancel?.();
                }}
              >
                {onCancel ? "Cancel" : "Start over"}
              </button>
            ) : (
              <span />
            )}
            {!showManualToken && (
              <button
                type="button"
                className="text-[11px] text-muted-foreground hover:text-foreground underline transition-colors"
                onClick={() => setShowManualToken(true)}
              >
                Having trouble? Paste token
              </button>
            )}
          </div>

          {showManualToken && (
            <div className="space-y-2 pt-1">
              <p className="text-[12px] font-semibold text-foreground">Session token (fallback)</p>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  placeholder="Paste token from browser…"
                  value={token}
                  onChange={(e) => {
                    setToken(e.target.value);
                    setTokenError("");
                  }}
                  className="input flex-1 text-[12px] font-mono"
                />
                <button
                  onClick={() => void handleTokenSubmit()}
                  disabled={oauthPhase === "submitting" || !token.trim()}
                  className="btn-primary !py-1.5 !px-4 !text-[12px] flex items-center gap-1.5 flex-shrink-0"
                >
                  {oauthPhase === "submitting" ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <Check size={12} />
                  )}
                  Connect
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {oauthPhase === "error" && tokenError && (
        <div className="space-y-2">
          <div
            className="rounded-lg px-3 py-2 text-[12px]"
            style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
          >
            {tokenError}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void startOAuth(serverUrl)}
              className={secondaryBtnClass}
            >
              <RotateCcw size={12} />
              Try again
            </button>
            {onCancel && (
              <button
                type="button"
                onClick={() => {
                  void resetOAuth();
                  onCancel();
                }}
                className={secondaryBtnClass}
              >
                <X size={12} />
                Cancel
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
