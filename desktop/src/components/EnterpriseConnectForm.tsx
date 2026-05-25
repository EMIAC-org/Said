import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Clock, ExternalLink, Link, Loader2, RotateCcw, X } from "lucide-react";
import { openExternal } from "@/lib/invoke";
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
}

export function EnterpriseConnectForm({
  onConnected,
  initialServerUrl = "",
  compact = false,
  variant = "default",
  onCancel,
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
  const serverUrlRef = useRef(serverUrl);
  serverUrlRef.current = serverUrl;

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
    setOauthPhase("waiting");
    await openExternal(larkUrl);
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
        await startOAuth(trimmed);
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
          Connect to your organization&apos;s AirNote Enterprise server. You must sign in with
          your workspace before using the app.
        </p>
      )}

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
            <button
              onClick={() => void handleValidate()}
              disabled={validating || waitingForBrowser || !serverUrl.trim()}
              className={connectBtnClass}
            >
              {validating ? (
                <Loader2 size={14} className="animate-spin" />
              ) : validated && !waitingForBrowser ? (
                <Check size={14} />
              ) : null}
              {waitingForBrowser
                ? "Waiting for sign-in…"
                : validated
                  ? "Verified — sign in below"
                  : "Connect workspace"}
            </button>
            <RecentWorkspaces />
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
              <button
                onClick={() => void handleValidate()}
                disabled={validating || waitingForBrowser || !serverUrl.trim()}
                className={connectBtnClass}
              >
                {validating ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : validated && !waitingForBrowser ? (
                  <Check size={12} />
                ) : null}
                {waitingForBrowser ? "Waiting…" : validated ? "Verified" : "Connect"}
              </button>
            </div>
            {recentUrls.length > 0 && <RecentWorkspaces />}
          </div>
        )}
      </div>

      {validationError && (
        <div
          className="rounded-lg px-3 py-2 text-[12px]"
          style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
        >
          {validationError}
        </div>
      )}

      {validated && !waitingForBrowser && oauthPhase === "idle" && (
        <div
          className="rounded-lg px-3 py-2 text-[12px] flex items-center gap-2"
          style={{ background: "hsl(145 60% 12%)", color: "hsl(145 70% 70%)" }}
        >
          <Check size={12} />
          Server is reachable. Sign in to complete the connection.
        </div>
      )}

      {waitingForBrowser && (
        <div className="space-y-3 pt-1">
          <div
            className="rounded-lg px-3 py-3 text-[12px] space-y-2"
            style={{ background: "hsl(210 60% 12%)", color: "hsl(210 70% 70%)" }}
          >
            <div className="flex items-center gap-2">
              {oauthPhase === "submitting" ? (
                <Loader2 size={14} className="shrink-0 animate-spin" />
              ) : (
                <ExternalLink size={14} className="shrink-0" />
              )}
              <span>
                {oauthPhase === "submitting"
                  ? "Finishing sign-in…"
                  : "Waiting for browser sign-in…"}
              </span>
            </div>
            <p className="text-[11px] opacity-80 leading-relaxed">
              Complete Lark sign-in in your browser. AirNote will connect automatically when
              you&apos;re done — no copy/paste needed.
            </p>
            {authUrl && oauthPhase === "waiting" && (
              <button
                type="button"
                className="text-[11px] opacity-80 hover:opacity-100 truncate text-left underline"
                onClick={() => void openExternal(authUrl)}
                title={authUrl}
              >
                Didn&apos;t open? Click here to reopen sign-in
              </button>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {oauthPhase === "waiting" && (
              <button
                type="button"
                onClick={() => void startOAuth(serverUrl)}
                className={secondaryBtnClass}
              >
                <RotateCcw size={12} />
                Try again
              </button>
            )}
            {onCancel && oauthPhase !== "submitting" && (
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

          {!showManualToken ? (
            <button
              type="button"
              onClick={() => setShowManualToken(true)}
              className="text-[11px] text-muted-foreground hover:text-foreground underline transition-colors"
            >
              Paste token manually
            </button>
          ) : (
            <div className="space-y-2 pt-1">
              <p className="text-[12px] font-semibold text-foreground">
                Session token (fallback)
              </p>
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
