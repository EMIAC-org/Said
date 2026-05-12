# Windows release pipeline — operational checklist

This document covers everything humans (not Claude / not CI) need to do
once the P5 code prep is merged. The code-side scaffolding is in place;
the operational steps below produce a release-ready pipeline.

## Required secrets + variables

### GitHub repo settings → Secrets and variables → Actions

| Type | Name | Value | Used by |
|---|---|---|---|
| Secret | `WINDOWS_PFX_BASE64` | `base64 -i cert.pfx` of the code-signing certificate | `release-windows.yml` |
| Secret | `WINDOWS_PFX_PASSWORD` | The .pfx password | `release-windows.yml` |
| Secret | `TAURI_SIGNING_PRIVATE_KEY` | Output of `cargo tauri signer generate` (private half) | `release-windows.yml` + `release.yml` (mac) |
| Secret | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Passphrase for the Tauri signer key | both release jobs |
| Secret | `UPDATE_MANIFEST_TRIGGER_TOKEN` | Random 32-byte hex; matches Cloudflare Worker `TRIGGER_TOKEN` | both release jobs |
| Variable | `TAURI_UPDATER_PUBKEY` | Public half of the Tauri signer key (multi-line PEM-style) | injected into `tauri.conf.json` at build time |
| Variable | `UPDATE_MANIFEST_TRIGGER_URL` | `https://said.emiac.com/trigger` | both release jobs |

## One-time setup steps

### 1. Procure a code-signing certificate

**Recommended path — SignPath.io (free for OSS):**
1. Apply at https://signpath.io for the open-source program.
2. Vetting takes ~1 week. Provide the GitHub repo URL.
3. Once approved, SignPath issues an EV-equivalent certificate signed
   by their sub-CA. Export as `.pfx` with a password.
4. base64-encode the `.pfx` and add as `WINDOWS_PFX_BASE64`.

**Alternative — DigiCert OV (~$200/yr):**
1. Buy an OV (organization-validated) code-signing cert.
2. Vetting: 1–2 weeks (DigiCert verifies the EMIAC organization).
3. EV variants (~$500/yr + cloud HSM ~$50/mo) bypass SmartScreen
   reputation accumulation but require either a USB token (won't work
   in CI) or a cloud HSM (DigiCert KeyLocker).

**During the SmartScreen reputation buildup** (first ~3000 installs
with an OV cert), users see a "Windows protected your PC" prompt with
"More info → Run anyway". EV certs skip this entirely.

### 2. Generate the Tauri updater keypair

```sh
cargo install tauri-cli
cargo tauri signer generate -w ./tauri-updater-key
# Outputs:
#   ./tauri-updater-key      (private — store as TAURI_SIGNING_PRIVATE_KEY)
#   ./tauri-updater-key.pub  (public — store as TAURI_UPDATER_PUBKEY variable)
```

Use a passphrase when prompted; store it as
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

`release-windows.yml` injects the public key into `tauri.conf.json`
at build time, replacing the `TAURI_UPDATER_PUBKEY_PLACEHOLDER` token.
The private key signs every release artifact; Tauri emits a `.sig`
file alongside each installer that the auto-updater verifies before
applying.

### 3. Provision the Cloudflare Worker

See `infrastructure/cloudflare-worker/README.md` for the full setup.
TL;DR:

```sh
cd infrastructure/cloudflare-worker
wrangler kv:namespace create UPDATES
wrangler kv:namespace create UPDATES --preview
# paste returned IDs into wrangler.toml
wrangler secret put TRIGGER_TOKEN     # paste the matching repo secret
wrangler deploy
# add route said.emiac.com/updates/* in the Cloudflare dashboard
```

### 4. Verify end-to-end

Cut a no-op test tag:

```sh
git tag v3.0.0-test
git push origin v3.0.0-test
```

Watch `release-windows.yml` run; confirm:
- [ ] said-backend sidecar builds
- [ ] Frontend Vite build succeeds
- [ ] `cargo tauri build` produces both `.msi` and `-setup.exe`
- [ ] `signtool verify /pa` passes
- [ ] GitHub Release is created with all artifacts + `.sig` files
- [ ] Worker `/trigger` returns 200 and updates `KV[latest:stable]`
- [ ] `curl https://said.emiac.com/updates/windows-x86_64/2.0.0` returns the JSON manifest

Then delete the test tag + release:

```sh
git push --delete origin v3.0.0-test
gh release delete v3.0.0-test
```

## Cutting a real release

```sh
just bump 3.1.0    # bumps workspace Cargo.toml + tauri.conf.json + package.json
git commit -am 'chore: bump version to 3.1.0'
git tag v3.1.0
git push origin main --tags
```

Both `release.yml` (mac) and `release-windows.yml` run in parallel,
upload their artifacts to the same GitHub Release, and trigger the
update manifest regen. Auto-update clients pick up the new version
on their next ~6h check (or immediately via Settings → "Check for
updates").

## Rollback

A bad release can be pulled in two ways:

1. **Stop new downloads only** (already-installed users stay on the bad
   version):
   ```sh
   wrangler kv:key put --binding=UPDATES paused true
   ```
   Clients receive 204 on every update check until you set `paused`
   back to `false`.

2. **Roll back to previous version** (force already-installed users
   onto the older binary): change the KV manifest manually:
   ```sh
   wrangler kv:key put --binding=UPDATES latest:stable "$(curl -s https://said.emiac.com/updates/windows-x86_64/0.0.0)"
   # or paste a known-good manifest JSON
   ```
   This rarely works as a true downgrade — Tauri's semver check
   considers older versions "not newer" and won't apply. Practical
   fix is to bump a `.1` hotfix release and push that.

## Failure modes

- **Cert expired**: `signtool sign` fails with error 0x80092004; CI
  job fails; release artifacts aren't published. Renew cert + push
  a new tag.
- **SmartScreen flags the installer**: only happens with unsigned or
  freshly-signed-without-reputation builds. Pre-submit each release
  to https://www.microsoft.com/wdsi/filesubmission to accelerate
  whitelisting (Defender team usually responds within 24h).
- **WebView2 install fails**: bootstrapper requires internet during
  install. Offer the offline installer variant as a fallback on the
  docs page (Tauri can produce both with `webviewInstallMode: offlineInstaller`).
