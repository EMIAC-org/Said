const STORAGE_KEY = "said:enterprise";
const PENDING_SERVER_URL_KEY = "said:enterprise-pending-url";
const RECENT_WORKSPACES_KEY = "said:enterprise-recent-urls";
const DEVICE_ID_FALLBACK_KEY = "said:enterprise-device-id";
const MAX_RECENT_WORKSPACES = 5;

/** Default AirNote cloud server for personal (non-org) accounts. */
export const DEFAULT_CLOUD_SERVER_URL = "https://airnote.emiactech.com";

function fallbackDeviceId(): string {
  try {
    const existing = localStorage.getItem(DEVICE_ID_FALLBACK_KEY);
    if (existing?.trim()) return existing.trim();
    const id = crypto.randomUUID();
    localStorage.setItem(DEVICE_ID_FALLBACK_KEY, id);
    return id;
  } catch {
    return "unknown-device";
  }
}

async function clientPayload(): Promise<{
  device_id: string;
  platform: string;
  app_version: string;
  hostname?: string;
  company_bucket_version?: number;
  company_vocab_synced_at?: string | null;
  personal_vocab_count?: number;
  personal_alias_count?: number;
}> {
  const platform =
    typeof navigator !== "undefined" && /Win/i.test(navigator.userAgent)
      ? "windows"
      : "macos";

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const { getVersion } = await import("@tauri-apps/api/app");
    const deviceId = await invoke<string>("get_device_id");
    const appVersion = await getVersion();
    try {
      localStorage.setItem(DEVICE_ID_FALLBACK_KEY, deviceId);
    } catch {
      // ignore
    }
    let hostname: string | undefined;
    try {
      hostname = await invoke<string>("get_hostname");
    } catch {
      hostname = undefined;
    }
    const vocab = await localCompanyVocabStatus();
    return {
      device_id: deviceId,
      platform,
      app_version: appVersion,
      hostname,
      company_bucket_version: vocab?.bucket?.version ?? undefined,
      company_vocab_synced_at: msToIso(vocab?.bucket?.last_synced_at),
      personal_vocab_count: vocab?.bucket?.term_count ?? undefined,
      personal_alias_count: vocab?.bucket?.alias_count ?? undefined,
    };
  } catch (err) {
    console.warn("[enterprise] clientPayload fallback", err);
    return {
      device_id: fallbackDeviceId(),
      platform,
      app_version: "unknown",
    };
  }
}

function msToIso(value: unknown): string | null | undefined {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return undefined;
  return new Date(value).toISOString();
}

async function localBackendFetch(path: string, opts: RequestInit = {}): Promise<Response | null> {
  try {
    const { getBackendEndpoint } = await import("./invoke");
    const endpoint = await getBackendEndpoint();
    if (!endpoint?.url || !endpoint.secret) return null;
    const headers: Record<string, string> = {
      ...(opts.headers as Record<string, string> | undefined),
      Authorization: `Bearer ${endpoint.secret}`,
    };
    if (opts.body && !headers["Content-Type"]) headers["Content-Type"] = "application/json";
    return fetch(`${endpoint.url}${path}`, { ...opts, headers });
  } catch (err) {
    console.warn("[enterprise] local backend fetch failed", err);
    return null;
  }
}

async function localCompanyVocabStatus(): Promise<any | null> {
  const res = await localBackendFetch("/v1/company-vocab/status");
  if (!res?.ok) return null;
  try {
    return await res.json();
  } catch {
    return null;
  }
}

export async function syncCompanyVocab(force = false): Promise<void> {
  const res = await localBackendFetch("/v1/company-vocab/sync", {
    method: "POST",
    body: JSON.stringify({ force }),
  });
  if (!res?.ok) return;
  try {
    const data = await res.json();
    if (data?.changed) console.info("[enterprise] company vocabulary synced", data.bucket);
  } catch {
    // ignore
  }
}

export async function uploadUserVocabSummary(force = false): Promise<void> {
  const payload = await clientPayload();
  const res = await localBackendFetch("/v1/company-vocab/upload-user-summary", {
    method: "POST",
    body: JSON.stringify({ device_id: payload.device_id, force }),
  });
  if (!res?.ok) return;
  try {
    const data = await res.json();
    if (data?.ok) console.info("[enterprise] uploaded vocab summary", data);
  } catch {
    // ignore
  }
}

function normalizeServerUrl(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, "");
  try {
    const parsed = new URL(trimmed);
    if (parsed.pathname === "" || parsed.pathname === "/") return parsed.origin;
    if (parsed.pathname.startsWith("/admin") || parsed.pathname.startsWith("/v1")) {
      return parsed.origin;
    }
  } catch {
    // Fall through to the original trimmed value.
  }
  return trimmed;
}

async function responseErrorMessage(res: Response, fallback: string): Promise<string> {
  const raw = await res.text().catch(() => "");
  if (!raw.trim()) return fallback;
  try {
    const parsed = JSON.parse(raw);
    if (typeof parsed?.error === "string" && parsed.error.trim()) return parsed.error;
    if (typeof parsed?.message === "string" && parsed.message.trim()) return parsed.message;
  } catch {
    // Non-JSON Axum extractor errors are still useful to show.
  }
  return raw.trim();
}

/** Recently used workspace server URLs (newest first). */
export function getRecentWorkspaceUrls(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_WORKSPACES_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((u): u is string => typeof u === "string" && u.trim().length > 0)
      .map(normalizeServerUrl);
  } catch {
    return [];
  }
}

/** Save a workspace URL to recents after successful validation or connect. */
export function rememberWorkspaceUrl(url: string): void {
  const normalized = normalizeServerUrl(url);
  if (!normalized) return;
  try {
    const next = [
      normalized,
      ...getRecentWorkspaceUrls().filter((u) => u !== normalized),
    ].slice(0, MAX_RECENT_WORKSPACES);
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // ignore
  }
}

/** Remove one URL from recents (optional clear in UI). */
export function forgetWorkspaceUrl(url: string): void {
  const normalized = normalizeServerUrl(url);
  try {
    const next = getRecentWorkspaceUrls().filter((u) => u !== normalized);
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // ignore
  }
}

export interface EnterpriseConnection {
  serverUrl: string;
  jwt: string;
  accountId: string;
  email: string;
  orgName?: string;
  activeOrgId?: string;
  larkName?: string;
  larkAvatarUrl?: string;
  authSource?: "lark" | "email";
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function isUuid(value: string | null | undefined): value is string {
  return typeof value === "string" && UUID_RE.test(value);
}

export async function fetchAuthenticatedAccount(
  serverUrl: string,
  jwt: string,
): Promise<{ id: string; email: string }> {
  const url = normalizeServerUrl(serverUrl);
  const res = await fetch(`${url}/v1/auth/me`, {
    headers: { Authorization: `Bearer ${jwt}` },
  });
  if (!res.ok) {
    throw new Error(await responseErrorMessage(res, "Invalid or expired token"));
  }
  const data = await res.json();
  const id = data?.account?.id;
  const email = data?.account?.email;
  if (!isUuid(id) || typeof email !== "string") {
    throw new Error("Server did not return a valid account");
  }
  return { id, email };
}

export async function repairEnterpriseConnection(
  conn: EnterpriseConnection,
): Promise<EnterpriseConnection> {
  const normalizedServerUrl = normalizeServerUrl(conn.serverUrl);
  if (isUuid(conn.accountId) && normalizedServerUrl === conn.serverUrl) return conn;

  const account = await fetchAuthenticatedAccount(normalizedServerUrl, conn.jwt);
  const repaired: EnterpriseConnection = {
    ...conn,
    serverUrl: normalizedServerUrl,
    accountId: account.id,
    email: account.email || conn.email,
  };
  saveConnection(repaired);
  return repaired;
}

export interface WorkspaceMembership {
  id: string;
  name: string;
  slug: string;
  role: string;
  is_active: boolean;
}

export interface WorkspaceListResponse {
  orgs: WorkspaceMembership[];
  active_org_id: string | null;
  personal_mode: boolean;
}

export async function listWorkspaces(): Promise<WorkspaceListResponse | null> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<WorkspaceListResponse>("list_workspaces");
  } catch (err) {
    console.warn("[enterprise] listWorkspaces failed", err);
    return null;
  }
}

export async function activateWorkspace(orgId: string): Promise<boolean> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke<string>("activate_workspace", { orgId });
    const conn = getConnection();
    if (conn) {
      const org = (await listWorkspaces())?.orgs.find((o) => o.id === orgId);
      saveConnection({
        ...conn,
        activeOrgId: orgId,
        orgName: org?.name ?? conn.orgName,
      });
    }
    return true;
  } catch (err) {
    console.warn("[enterprise] activateWorkspace failed", err);
    return false;
  }
}

export async function deactivateWorkspace(): Promise<boolean> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("deactivate_workspace");
    const conn = getConnection();
    if (conn) {
      saveConnection({ ...conn, activeOrgId: undefined });
    }
    return true;
  } catch (err) {
    console.warn("[enterprise] deactivateWorkspace failed", err);
    return false;
  }
}

/** Header that scopes a server request to the connection's active workspace.
 *  The server's require_active_org gate accepts either this header or a
 *  previously-activated org, so sending it makes meeting calls robust even if
 *  the server-side active org drifts. Empty when no workspace is active. */
function orgHeader(): Record<string, string> {
  const conn = getConnection();
  return conn?.activeOrgId ? { "X-AirNote-Org-Id": conn.activeOrgId } : {};
}

/** Ensure the connection has an active workspace so org-scoped calls (meetings)
 *  don't 403 with "active workspace required". Returns true when a workspace is
 *  active (or was just auto-activated), false when the user must pick one.
 *
 *  Auto-activates only when the choice is unambiguous (exactly one workspace and
 *  not in personal mode); with zero or multiple workspaces we let the caller
 *  guide the user instead of guessing. */
export async function ensureActiveWorkspace(): Promise<boolean> {
  const conn = getConnection();
  const data = await listWorkspaces();
  // Couldn't reach the server — don't nag if a workspace was already active.
  if (!data) return !!conn?.activeOrgId;
  if (data.active_org_id) {
    if (conn && conn.activeOrgId !== data.active_org_id) {
      saveConnection({ ...conn, activeOrgId: data.active_org_id });
    }
    return true;
  }
  if (!data.personal_mode && data.orgs.length === 1) {
    return await activateWorkspace(data.orgs[0].id);
  }
  return false;
}

interface LocalEnterpriseStatus {
  connected: boolean;
  license_tier?: string;
  email?: string | null;
  server_url?: string | null;
  org_name?: string | null;
  token?: string | null;
}

export type ConnectionStatus = "connected" | "missing" | "expired";

/** Check if connected to an enterprise server */
export function isConnected(): boolean {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    return stored !== null;
  } catch {
    return false;
  }
}

/** Get current connection info, or null */
export function getConnection(): EnterpriseConnection | null {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (!stored) return null;
    return JSON.parse(stored);
  } catch {
    return null;
  }
}

/** Save connection info after successful OAuth */
export function saveConnection(conn: EnterpriseConnection): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(conn));
  rememberWorkspaceUrl(conn.serverUrl);
  try {
    localStorage.removeItem(PENDING_SERVER_URL_KEY);
  } catch {
    // ignore
  }
}

/** Clear connection (disconnect) */
export function disconnect(): void {
  localStorage.removeItem(STORAGE_KEY);
}

export async function restoreConnectionFromLocalBackend(): Promise<EnterpriseConnection | null> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const status = await invoke<LocalEnterpriseStatus>("get_enterprise_status");
    if (!status.connected || !status.token || !status.server_url || !status.email) {
      return null;
    }
    const existing = getConnection();
    if (existing?.jwt === status.token && existing.serverUrl === status.server_url) {
      return repairEnterpriseConnection(existing).catch(() => existing);
    }
    const conn: EnterpriseConnection = {
      serverUrl: normalizeServerUrl(status.server_url),
      jwt: status.token,
      accountId: existing?.accountId ?? "local-backend",
      email: status.email,
      orgName: status.org_name ?? undefined,
      larkName: existing?.larkName,
      larkAvatarUrl: existing?.larkAvatarUrl,
      authSource: existing?.authSource ?? "email",
    };
    return repairEnterpriseConnection(conn).catch(() => {
      saveConnection(conn);
      return conn;
    });
  } catch (err) {
    console.warn("[enterprise] restore from local backend failed", err);
    return null;
  }
}

/** Remember server URL across OAuth round-trip */
export function setPendingServerUrl(url: string): void {
  try {
    localStorage.setItem(PENDING_SERVER_URL_KEY, url);
  } catch {
    // ignore
  }
}

export function getPendingServerUrl(): string | null {
  try {
    return localStorage.getItem(PENDING_SERVER_URL_KEY);
  } catch {
    return null;
  }
}

/** Validate stored session against the server; clear on expiry. */
export async function checkConnection(): Promise<ConnectionStatus> {
  const conn = getConnection();
  if (!conn?.jwt || !conn.serverUrl) return "missing";

  try {
    const url = conn.serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/auth/me`, {
      headers: { Authorization: `Bearer ${conn.jwt}` },
    });
    if (res.ok) return "connected";
    if (res.status === 401 || res.status === 403) {
      disconnect();
      return "expired";
    }
    // Network/server errors — trust local cache for offline grace
    return "connected";
  } catch {
    return "connected";
  }
}

/** Validate server URL by hitting its health endpoint */
export async function validateServer(serverUrl: string): Promise<boolean> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    setPendingServerUrl(url);
    const res = await fetch(`${url}/v1/health`);
    if (!res.ok) return false;
    const data = await res.json();
    if (data.ok === true) {
      rememberWorkspaceUrl(url);
      return true;
    }
    return false;
  } catch {
    return false;
  }
}

/** Register this desktop install with the enterprise server. */
export async function registerClient(serverUrl: string, jwt: string): Promise<void> {
  const url = serverUrl.replace(/\/+$/, "");
  const body = await clientPayload();
  const res = await fetch(`${url}/v1/clients/register`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${jwt}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok && res.status !== 204) {
    const detail = await res.text().catch(() => "");
    throw new Error(
      `Failed to register desktop client (${res.status})${detail ? `: ${detail}` : ""}`,
    );
  }
}

/** Heartbeat — refresh last_seen_at on the server. */
export async function sendHeartbeat(serverUrl: string, jwt: string): Promise<void> {
  const url = serverUrl.replace(/\/+$/, "");
  const body = await clientPayload();
  const res = await fetch(`${url}/v1/clients/heartbeat`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${jwt}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok && res.status !== 204) {
    const detail = await res.text().catch(() => "");
    throw new Error(
      `Failed to send desktop heartbeat (${res.status})${detail ? `: ${detail}` : ""}`,
    );
  }
}

/** Register on connect; fall back to heartbeat upsert if register fails. */
export async function ensureDesktopRegistered(
  serverUrl: string,
  jwt: string,
): Promise<boolean> {
  if (!serverUrl.trim() || !jwt.trim()) return false;
  try {
    await registerClient(serverUrl, jwt);
    console.info("[enterprise] desktop registered with server");
    return true;
  } catch (err) {
    console.warn("[enterprise] register failed, trying heartbeat", err);
  }
  try {
    await sendHeartbeat(serverUrl, jwt);
    console.info("[enterprise] desktop heartbeat sent");
    return true;
  } catch (err) {
    console.error("[enterprise] heartbeat failed", err);
    return false;
  }
}

/** Verify org license is active before proceeding. */
export async function checkLicense(serverUrl: string, jwt: string): Promise<boolean> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/license/check`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (!res.ok) return false;
    const data = await res.json();
    return data.active !== false;
  } catch {
    return true;
  }
}

/** Complete enterprise auth by validating a session token from the Lark OAuth callback. */
export async function completeAuth(
  serverUrl: string,
  sessionToken: string,
): Promise<EnterpriseConnection> {
  const url = serverUrl.replace(/\/+$/, "");

  const meRes = await fetch(`${url}/v1/auth/me`, {
    headers: { Authorization: `Bearer ${sessionToken}` },
  });
  if (!meRes.ok) throw new Error("Invalid or expired token");
  const me = await meRes.json();

  const orgRes = await fetch(`${url}/v1/orgs/me`, {
    headers: { Authorization: `Bearer ${sessionToken}` },
  });
  let orgName: string | undefined;
  let larkName: string | undefined;
  let larkAvatarUrl: string | undefined;
  if (orgRes.ok) {
    const orgData = await orgRes.json();
    orgName = orgData.org?.name;
    larkName = orgData.org?.lark_name;
    larkAvatarUrl = orgData.org?.lark_avatar_url;
  }

  const licenseOk = await checkLicense(url, sessionToken);
  if (!licenseOk) {
    throw new Error("Your workspace license is inactive. Contact your administrator.");
  }

  const conn: EnterpriseConnection = {
    serverUrl: url,
    jwt: sessionToken,
    accountId: me.account.id,
    email: me.account.email,
    orgName,
    larkName,
    larkAvatarUrl,
    authSource: larkName ? "lark" : "email",
  };

  await persistEnterpriseConnection(conn, sessionToken, me.account.email, orgName);

  return conn;
}

export async function completeEmailAuth(
  serverUrl: string,
  email: string,
  password: string,
  signup: boolean,
): Promise<EnterpriseConnection> {
  const url = serverUrl.replace(/\/+$/, "");
  const res = await fetch(`${url}/v1/auth/desktop-email`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password, signup }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.error || "Email sign-in failed");
  }

  const sessionToken = data.token as string;
  const accountEmail = data.account?.email as string;

  const orgRes = await fetch(`${url}/v1/orgs/me`, {
    headers: { Authorization: `Bearer ${sessionToken}` },
  });
  let orgName: string | undefined;
  if (orgRes.ok) {
    const orgData = await orgRes.json();
    orgName = orgData.org?.name;
  }

  const licenseOk = await checkLicense(url, sessionToken);
  if (!licenseOk) {
    throw new Error("Your workspace license is inactive. Contact your administrator.");
  }

  const conn: EnterpriseConnection = {
    serverUrl: url,
    jwt: sessionToken,
    accountId: data.account?.id,
    email: accountEmail,
    orgName,
    authSource: "email",
  };

  await persistEnterpriseConnection(conn, sessionToken, accountEmail, orgName);

  return conn;
}

async function persistEnterpriseConnection(
  conn: EnterpriseConnection,
  sessionToken: string,
  email: string,
  orgName?: string,
): Promise<void> {
  saveConnection(conn);

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("store_enterprise_auth", {
      token: sessionToken,
      email,
      serverUrl: conn.serverUrl,
      orgName: orgName ?? null,
    });
  } catch {
    // Non-fatal for UI; backend gate may fail until retried
  }

  try {
    await ensureDesktopRegistered(conn.serverUrl, sessionToken);
    await syncCompanyVocab(true);
    await uploadUserVocabSummary(true);
    const { syncCredentialVault } = await import("./invoke");
    const vault = await syncCredentialVault();
    if (vault?.failed) {
      console.warn("[enterprise] credential vault sync partial failure", vault);
    }
  } catch (err) {
    console.warn("[enterprise] desktop registration deferred until next heartbeat", err);
  }
}

/** Full disconnect — local storage + backend token. */
export async function disconnectEnterprise(): Promise<void> {
  disconnect();
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("clear_enterprise_auth");
  } catch {
    // ignore
  }
}

/** Get user's org info */
export async function getMyOrg(
  serverUrl: string,
  jwt: string,
): Promise<{ id: string; name: string; slug: string; role: string } | null> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/orgs/me`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (!res.ok) return null;
    const data = await res.json();
    return data.org;
  } catch {
    return null;
  }
}

/** List meetings for the user's org */
export async function listMeetings(
  serverUrl: string,
  jwt: string,
  status?: string,
): Promise<any[]> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    const qs = status ? `?status=${status}` : "";
    const res = await fetch(`${url}/v1/meetings${qs}`, {
      headers: { Authorization: `Bearer ${jwt}`, ...orgHeader() },
    });
    if (!res.ok) return [];
    const data = await res.json();
    return data.meetings ?? [];
  } catch {
    return [];
  }
}

/** Create a meeting in the user's org */
export async function createMeeting(
  serverUrl: string,
  jwt: string,
  body: {
    title: string;
    agenda?: string | null;
    participant_ids: string[];
    scheduled_at?: string | null;
    duration_minutes?: number;
  },
): Promise<any> {
  const url = normalizeServerUrl(serverUrl);
  const res = await fetch(`${url}/v1/meetings`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${jwt}`,
      "Content-Type": "application/json",
      ...orgHeader(),
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(await responseErrorMessage(res, "Failed to create meeting"));
  }
  const data = await res.json().catch(() => ({}));
  return data.meeting;
}

/** Mark a meeting live */
export async function startMeeting(
  serverUrl: string,
  jwt: string,
  meetingId: string,
): Promise<void> {
  const url = normalizeServerUrl(serverUrl);
  const res = await fetch(`${url}/v1/meetings/${meetingId}/start`, {
    method: "POST",
    headers: { Authorization: `Bearer ${jwt}`, ...orgHeader() },
  });
  if (!res.ok) {
    throw new Error(await responseErrorMessage(res, "Failed to start meeting"));
  }
}

// ── OpenAI integration ───────────────────────────────────────────────────────

export interface OpenAIStatus {
  connected: boolean;
  plan_type?: string;
  label?: string;
  connected_at?: string;
}

/** Get current OpenAI connection status */
export async function getOpenAIStatus(): Promise<OpenAIStatus | null> {
  try {
    const conn = getConnection();
    if (!conn) return null;
    const url = conn.serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/openai/status`, {
      headers: { Authorization: `Bearer ${conn.jwt}` },
    });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

/** Initiate OpenAI PKCE OAuth — returns auth_url, code_verifier, state */
export async function initiateOpenAIConnect(): Promise<{
  auth_url: string;
  code_verifier: string;
  state: string;
} | null> {
  try {
    const conn = getConnection();
    if (!conn) return null;
    const url = conn.serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/openai/connect`, {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.jwt}` },
    });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

/** Complete OpenAI OAuth by exchanging the authorization code */
export async function completeOpenAIConnect(
  code: string,
  codeVerifier: string,
  planType?: string,
  label?: string,
): Promise<boolean> {
  try {
    const conn = getConnection();
    if (!conn) return false;
    const url = conn.serverUrl.replace(/\/+$/, "");
    const body: Record<string, string> = { code, code_verifier: codeVerifier };
    if (planType) body.plan_type = planType;
    if (label) body.label = label;
    const res = await fetch(`${url}/v1/openai/complete`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${conn.jwt}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    });
    if (!res.ok) return false;
    const data = await res.json();
    return data.connected === true;
  } catch {
    return false;
  }
}

/** Sync a completed meeting's AI results (tasks, doc, notifications) to Lark */
export async function syncMeetingToLark(meetingId: string): Promise<{
  tasks_synced: number;
  doc_id?: string;
  messages_sent: number;
} | null> {
  try {
    const conn = getConnection();
    if (!conn) return null;
    const url = conn.serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/meetings/${meetingId}/sync-to-lark`, {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.jwt}` },
    });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

export interface LarkExportPayload {
  title: string;
  summary: string;
  action_items: Array<{ title: string; assignee?: string | null }>;
  decisions: string[];
}

export type LarkExportResult =
  | { ok: true; url: string; inSharedFolder: boolean; warning?: string | null }
  | { ok: false; code: string; message: string };

/**
 * Export a meeting's locally-generated minutes to a Lark Docx document. The
 * desktop holds the content, so it is sent in the body; the control-plane
 * creates the doc with the org's Lark app. Returns a categorized result so the
 * UI can distinguish "not connected" / "not configured" / "API error" / etc.
 */
export async function exportMeetingToLark(
  meetingId: string,
  payload: LarkExportPayload,
): Promise<LarkExportResult> {
  const conn = getConnection();
  if (!conn) {
    return { ok: false, code: "not_connected", message: "Not connected to your workspace." };
  }
  const url = conn.serverUrl.replace(/\/+$/, "");
  try {
    const res = await fetch(`${url}/v1/meetings/${meetingId}/export-lark`, {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.jwt}`, "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (res.status === 401) {
      return { ok: false, code: "unauthorized", message: "Your session expired — reconnect and try again." };
    }
    const data = await res.json().catch(() => ({}) as Record<string, unknown>);
    // A 404 with no error body means the route isn't on the server (the handler
    // returns a JSON error for a missing meeting), i.e. the control-plane hasn't
    // been deployed with the export endpoint yet.
    if (res.status === 404 && typeof data.error !== "string") {
      return {
        ok: false,
        code: "endpoint_missing",
        message: "Lark export isn't available on your workspace server yet — it needs to be updated.",
      };
    }
    if (!res.ok) {
      return {
        ok: false,
        code: typeof data.code === "string" ? data.code : "error",
        message: typeof data.error === "string" ? data.error : `Lark export failed (${res.status}).`,
      };
    }
    return {
      ok: true,
      url: typeof data.url === "string" ? data.url : "",
      inSharedFolder: data.in_shared_folder === true,
      warning: typeof data.content_warning === "string" ? data.content_warning : null,
    };
  } catch {
    return { ok: false, code: "offline", message: "You're offline — try again." };
  }
}

/** Disconnect OpenAI account */
export async function disconnectOpenAI(): Promise<boolean> {
  try {
    const conn = getConnection();
    if (!conn) return false;
    const url = conn.serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/openai/disconnect`, {
      method: "DELETE",
      headers: { Authorization: `Bearer ${conn.jwt}` },
    });
    return res.status === 204 || res.ok;
  } catch {
    return false;
  }
}
