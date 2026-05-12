// Said update-manifest Worker.
//
// Routes:
//   GET  /updates/:target/:current_version   — Tauri updater endpoint
//   POST /trigger                            — CI cache invalidation
//   GET  /health                             — basic uptime probe
//
// Tauri updater contract: respond with 204 if the installed version is
// already current (or newer); otherwise return JSON with download URL,
// signature, version, notes, pub_date.
//
// Channels are encoded into the target path as `<channel>:<target>`,
// e.g. `stable:windows-x86_64` or `beta:darwin-aarch64`. Plain target
// strings default to the `DEFAULT_CHANNEL` (stable).

export interface Env {
  UPDATES: KVNamespace;
  GITHUB_REPO: string;
  DEFAULT_CHANNEL: string;
  TRIGGER_TOKEN?: string;
}

interface PlatformArtifact {
  signature: string;
  url: string;
}

interface UpdateManifest {
  version: string;
  notes: string;
  pub_date: string; // RFC 3339
  platforms: Record<string, PlatformArtifact>;
}

const CHANNELS = new Set(["stable", "beta", "nightly"]);

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const path = url.pathname;

    if (path === "/health") {
      return new Response("ok", { status: 200 });
    }

    if (path === "/trigger" && req.method === "POST") {
      return handleTrigger(req, env);
    }

    // Tauri endpoint: /updates/{target}/{current_version}
    const m = /^\/updates\/([^/]+)\/([^/]+)$/.exec(path);
    if (m && req.method === "GET") {
      const targetPart = decodeURIComponent(m[1]);
      const currentVersion = decodeURIComponent(m[2]);
      return handleUpdate(targetPart, currentVersion, env);
    }

    return new Response("Not found", { status: 404 });
  },
};

async function handleUpdate(
  targetPart: string,
  currentVersion: string,
  env: Env,
): Promise<Response> {
  // Honor global pause kill-switch.
  const paused = await env.UPDATES.get("paused");
  if (paused === "true") {
    return new Response(null, { status: 204 });
  }

  // Decode channel prefix if present (e.g. `beta:windows-x86_64`).
  let channel = env.DEFAULT_CHANNEL;
  let target = targetPart;
  const colon = targetPart.indexOf(":");
  if (colon > 0) {
    const candidate = targetPart.slice(0, colon);
    if (CHANNELS.has(candidate)) {
      channel = candidate;
      target = targetPart.slice(colon + 1);
    }
  }

  const key = `latest:${channel}`;
  const raw = await env.UPDATES.get(key);
  if (!raw) {
    return new Response(null, { status: 204 });
  }

  let manifest: UpdateManifest;
  try {
    manifest = JSON.parse(raw);
  } catch {
    return new Response(null, { status: 204 });
  }

  if (semverGte(currentVersion, manifest.version)) {
    return new Response(null, { status: 204 });
  }

  const platformEntry = manifest.platforms[target];
  if (!platformEntry) {
    return new Response(null, { status: 204 });
  }

  const body: UpdateManifest = {
    version: manifest.version,
    notes: manifest.notes,
    pub_date: manifest.pub_date,
    platforms: { [target]: platformEntry },
  };

  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      "content-type": "application/json",
      "cache-control": "public, max-age=300",
    },
  });
}

async function handleTrigger(req: Request, env: Env): Promise<Response> {
  if (!env.TRIGGER_TOKEN) {
    return new Response("trigger not configured", { status: 503 });
  }
  const auth = req.headers.get("authorization") ?? "";
  if (auth !== `Bearer ${env.TRIGGER_TOKEN}`) {
    return new Response("unauthorized", { status: 401 });
  }
  const payload = (await req.json().catch(() => ({}))) as {
    event?: string;
    tag?: string;
  };
  if (!payload.tag) {
    return new Response("missing tag", { status: 400 });
  }

  const channel = inferChannel(payload.tag);
  const manifest = await buildManifestFromGitHub(env, payload.tag);
  if (!manifest) {
    return new Response("manifest build failed", { status: 502 });
  }
  await env.UPDATES.put(`latest:${channel}`, JSON.stringify(manifest));

  return new Response(JSON.stringify({ channel, version: manifest.version }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function inferChannel(tag: string): string {
  if (tag.includes("-beta") || tag.includes("-rc")) return "beta";
  if (tag.includes("-nightly") || tag.startsWith("nightly-")) return "nightly";
  return "stable";
}

async function buildManifestFromGitHub(
  env: Env,
  tag: string,
): Promise<UpdateManifest | null> {
  const url = `https://api.github.com/repos/${env.GITHUB_REPO}/releases/tags/${encodeURIComponent(tag)}`;
  const resp = await fetch(url, {
    headers: {
      "user-agent": "said-update-worker",
      accept: "application/vnd.github+json",
    },
  });
  if (!resp.ok) return null;
  const release = (await resp.json()) as {
    name?: string;
    tag_name: string;
    body?: string;
    published_at: string;
    assets: { name: string; browser_download_url: string }[];
  };

  // Strip a leading "v" from the tag for the manifest version.
  const version = release.tag_name.replace(/^v/, "");
  const platforms: Record<string, PlatformArtifact> = {};

  const findAsset = (predicate: (n: string) => boolean) =>
    release.assets.find((a) => predicate(a.name));
  const sigFor = (artifactName: string) =>
    release.assets.find((a) => a.name === `${artifactName}.sig`)
      ?.browser_download_url ?? "";

  // Mac (.dmg or .app.tar.gz from Tauri updater bundle).
  const macAArch = findAsset(
    (n) => /aarch64/.test(n) && /\.(app\.tar\.gz|dmg)$/.test(n),
  );
  if (macAArch) {
    platforms["darwin-aarch64"] = {
      url: macAArch.browser_download_url,
      signature: await fetchTextOr("", sigFor(macAArch.name)),
    };
  }
  const macX64 = findAsset(
    (n) => /x86_64|x64/.test(n) && /\.(app\.tar\.gz|dmg)$/.test(n),
  );
  if (macX64) {
    platforms["darwin-x86_64"] = {
      url: macX64.browser_download_url,
      signature: await fetchTextOr("", sigFor(macX64.name)),
    };
  }

  // Windows (NSIS .exe).
  const winNsis = findAsset(
    (n) => /-setup\.exe$/.test(n) && !/aarch64/.test(n),
  );
  if (winNsis) {
    platforms["windows-x86_64"] = {
      url: winNsis.browser_download_url,
      signature: await fetchTextOr("", sigFor(winNsis.name)),
    };
  }

  if (Object.keys(platforms).length === 0) {
    return null;
  }

  return {
    version,
    notes: release.body ?? "",
    pub_date: release.published_at,
    platforms,
  };
}

async function fetchTextOr(fallback: string, url: string): Promise<string> {
  if (!url) return fallback;
  try {
    const r = await fetch(url, {
      headers: { "user-agent": "said-update-worker" },
    });
    if (!r.ok) return fallback;
    return (await r.text()).trim();
  } catch {
    return fallback;
  }
}

function semverGte(a: string, b: string): boolean {
  const parse = (s: string) => {
    const [core] = s.split("+");
    const [num, pre] = core.split("-");
    const parts = num.split(".").map((p) => Number.parseInt(p, 10) || 0);
    while (parts.length < 3) parts.push(0);
    return { parts, pre: pre ?? "" };
  };
  const A = parse(a);
  const B = parse(b);
  for (let i = 0; i < 3; i++) {
    if (A.parts[i] > B.parts[i]) return true;
    if (A.parts[i] < B.parts[i]) return false;
  }
  // Versions equal numerically — a prerelease is "less than" a release.
  if (A.pre === "" && B.pre !== "") return true;
  if (A.pre !== "" && B.pre === "") return false;
  return A.pre >= B.pre;
}
