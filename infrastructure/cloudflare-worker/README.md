# said-update-manifest Cloudflare Worker

Serves Tauri update manifests for Said. Routes:

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/updates/:target/:current_version` | Tauri updater endpoint |
| `POST` | `/trigger` | CI cache invalidation (auth: bearer token) |
| `GET` | `/health` | Liveness probe |

## Release channels

Channels are encoded into the `target` path with a colon prefix:

- `stable:windows-x86_64` (default channel can omit the prefix)
- `beta:darwin-aarch64`
- `nightly:windows-x86_64`

The Tauri client constructs the channel-aware target string from local
config (`SettingsView.tsx` → Updates section in P4.1).

## Kill switch

Set the KV key `paused` to the string `"true"` (case-sensitive) to make
the Worker return `204 No Content` for every update request. Useful when
a bad release ships and we need every client to stop updating until we
push a hotfix.

```sh
wrangler kv:key put --binding=UPDATES paused true
# undo:
wrangler kv:key put --binding=UPDATES paused false
```

## Setup (one-time)

1. **Create the KV namespace**:
   ```sh
   wrangler kv:namespace create UPDATES
   wrangler kv:namespace create UPDATES --preview
   ```
   Copy the resulting `id` + `preview_id` into `wrangler.toml`.

2. **Set the trigger token** (used by `release-{macos,windows}.yml`):
   ```sh
   wrangler secret put TRIGGER_TOKEN
   # paste a strong random token; mirror it as repo secret
   # UPDATE_MANIFEST_TRIGGER_TOKEN
   ```

3. **Deploy**:
   ```sh
   wrangler deploy
   ```

4. **Route the domain**: in the Cloudflare dashboard, add a route for
   `said.emiac.com/updates/*` pointing to this Worker. Add the host on
   the matching CNAME record if it doesn't already resolve.

5. **Register the trigger URL** as repo variable
   `UPDATE_MANIFEST_TRIGGER_URL` (e.g. `https://said.emiac.com/trigger`).

## How CI uses it

After `release-windows.yml` (and `release-macos.yml`) publish artifacts
to a GitHub Release, they `POST /trigger` with the tag. The Worker:

1. Fetches `https://api.github.com/repos/EMIAC-org/Said/releases/tags/<tag>`.
2. Builds an `UpdateManifest` from the asset list (matching `aarch64`,
   `x86_64`, `-setup.exe` patterns).
3. Reads adjacent `.sig` files (Tauri updater EdDSA signatures) and
   embeds the signature text into the manifest.
4. Writes the manifest to `KV[latest:<channel>]`.

Subsequent `/updates/...` requests serve the new manifest.

## Failure modes

- **Tag has no assets**: Worker returns 502 on `/trigger`; KV is not
  updated; clients keep getting the previous manifest. Re-run CI or
  upload artifacts manually + re-trigger.
- **EdDSA signature mismatch**: Tauri updater client rejects the
  download. Verify the public key in `tauri.conf.json` matches the
  private key in `TAURI_SIGNING_PRIVATE_KEY`.
- **Worker down**: clients receive an HTTP error, Tauri updater skips
  the check and tries again next interval. Safe default.

## Local dev

```sh
npm install -g wrangler
wrangler dev          # serves on http://127.0.0.1:8787
# Test:
curl http://127.0.0.1:8787/health
curl http://127.0.0.1:8787/updates/windows-x86_64/3.0.0
```
