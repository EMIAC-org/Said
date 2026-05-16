const STORAGE_KEY = "said:enterprise";

export interface EnterpriseConnection {
  serverUrl: string;
  jwt: string;
  accountId: string;
  email: string;
  orgName?: string;
  larkName?: string;
  larkAvatarUrl?: string;
}

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
}

/** Clear connection (disconnect) */
export function disconnect(): void {
  localStorage.removeItem(STORAGE_KEY);
}

/** Validate server URL by hitting its health endpoint */
export async function validateServer(serverUrl: string): Promise<boolean> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/health`);
    if (!res.ok) return false;
    const data = await res.json();
    return data.ok === true;
  } catch {
    return false;
  }
}

/** Complete enterprise auth by validating a session token from the Lark OAuth callback.
 *  Fetches user identity + org info and saves the connection. */
export async function completeAuth(serverUrl: string, sessionToken: string): Promise<EnterpriseConnection> {
  const url = serverUrl.replace(/\/+$/, "");

  // Validate token by fetching user identity
  const meRes = await fetch(`${url}/v1/auth/me`, {
    headers: { Authorization: `Bearer ${sessionToken}` },
  });
  if (!meRes.ok) throw new Error("Invalid or expired token");
  const me = await meRes.json();

  // Fetch org info
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

  // Fetch lark profile from org members
  if (orgName) {
    try {
      const membersRes = await fetch(`${url}/v1/orgs/me`, {
        headers: { Authorization: `Bearer ${sessionToken}` },
      });
      if (membersRes.ok) {
        const md = await membersRes.json();
        if (md.org?.lark_name) larkName = md.org.lark_name;
        if (md.org?.lark_avatar_url) larkAvatarUrl = md.org.lark_avatar_url;
      }
    } catch {}
  }

  const conn: EnterpriseConnection = {
    serverUrl: url,
    jwt: sessionToken,
    accountId: me.account.id,
    email: me.account.email,
    orgName,
    larkName,
    larkAvatarUrl,
  };

  saveConnection(conn);

  // Store in local said-backend so the app profile shows signed-in state
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("store_enterprise_auth", { token: sessionToken, email: me.account.email });
  } catch {}

  return conn;
}

/** Get user's org info */
export async function getMyOrg(serverUrl: string, jwt: string): Promise<{ id: string; name: string; slug: string; role: string } | null> {
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
export async function listMeetings(serverUrl: string, jwt: string, status?: string): Promise<any[]> {
  try {
    const url = serverUrl.replace(/\/+$/, "");
    const qs = status ? `?status=${status}` : "";
    const res = await fetch(`${url}/v1/meetings${qs}`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (!res.ok) return [];
    const data = await res.json();
    return data.meetings ?? [];
  } catch {
    return [];
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
export async function initiateOpenAIConnect(): Promise<{ auth_url: string; code_verifier: string; state: string } | null> {
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
