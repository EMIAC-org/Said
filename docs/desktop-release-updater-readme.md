# AirNote desktop release and auto-update runbook

This README explains the complete desktop update pipeline: building a macOS
release, Apple signing/notarization, publishing versioned artifacts to the VM,
how the installed app discovers and stages an update, and how to safely reuse
the release setup in another project.

It is based on the repository and a read-only inspection of production on
2026-07-24. No credentials are recorded here.

## The two distributables

A normal macOS release has two distinct payloads:

1. **Manual installer:** a signed and notarized DMG that a person downloads,
   opens, and drags into /Applications.
2. **In-app updater payload:** a separately signed AirNote.app.tar.gz archive
   that Tauri verifies and installs after the user chooses Restart.

The DMG is not the updater payload. Both are required for a proper release.

| Need | Command | Changes public release files? |
| --- | --- | --- |
| Build a distributable Mac release | just dmg | No; local build only |
| Build a test Mac DMG | just local-dmg | No; local build only |
| Bump all pinned versions | just bump 2.4.6 | No; source files only |
| Publish an already-built Mac release | ./scripts/deploy-release-vm.sh aarch64-apple-darwin | Yes |
| Tag main after release is committed | just release 2.4.6 | Pushes main and v2.4.6 |

Current supported release target: aarch64-apple-darwin (Apple Silicon).
Windows has a separate manifest and must not be changed during a Mac release.

## End-to-end map

~~~text
local Mac                                                     production VM

Cargo.toml version
    |
    +--> just dmg --> AirNote.app --> signed + notarized DMG ----> /releases/<version>/*.dmg
                     |
                     +--> updater archive + minisign signature --> /releases/<version>/*.app.tar.gz
                                                                  /updates/darwin/latest.json

installed app --> checks signed manifest --> downloads archive --> user clicks Restart
              --> signature-authenticated install --> relaunches updated AirNote
~~~

The production URLs follow this convention:

~~~text
Updater manifest:
https://airnote.emiactech.com/updates/darwin/latest.json

Manual DMG:
https://airnote.emiactech.com/releases/<version>/AirNote_<version>_aarch64.dmg

Updater archive:
https://airnote.emiactech.com/releases/<version>/AirNote_<version>_aarch64.app.tar.gz
~~~

The current verified production Darwin release is 2.4.5. Its manifest,
manual DMG, and updater archive all returned HTTP 200 during this review.

## How AirNote discovers and applies updates

### Trust model and endpoint order

desktop/src-tauri/tauri.conf.json embeds the Tauri updater public key. The
installed app has this trust anchor before it contacts the network.

It configures these endpoints in order:

1. production target-specific VM manifest
2. target-specific GitHub release manifest
3. legacy combined VM manifest
4. legacy combined GitHub manifest

For Apple Silicon, the platform key inside the manifest is `darwin-aarch64`.
Tauri's `{{target}}` URL placeholder is only the operating-system name,
`darwin` (the architecture is handled separately), so the configured primary
endpoint resolves to the canonical public Darwin manifest:

~~~text
https://airnote.emiactech.com/updates/darwin/latest.json
~~~

The published manifest carries a semantic version and platform-specific archive
URL and signature, conceptually:

~~~json
{
  "version": "2.4.5",
  "platforms": {
    "darwin-aarch64": {
      "signature": "<minisign signature>",
      "url": "https://.../AirNote_2.4.5_aarch64.app.tar.gz"
    }
  }
}
~~~

The version and URL alone are not trusted. The app only installs an updater
archive that verifies against its embedded public key. This is separate from:

- **Developer ID signing:** identifies the macOS publisher.
- **Apple notarization/stapling:** lets Gatekeeper trust the downloaded DMG.
- **Tauri updater signature:** authenticates the archive that may replace an
  already-installed app.

### Polling policy

desktop/src/App.tsx starts startDailyAutoUpdateCheck() when the main React
webview mounts. Its implementation lives in desktop/src/lib/autoUpdate.ts.

The policy is intentionally quiet and non-blocking:

1. Wait 10 seconds after application startup.
2. Wake again hourly and on focus, visibility, online, and pageshow events.
   The foreground signals make laptop wake-up responsive.
3. Read airnote:auto-update:last-check-ms from localStorage. Unless an update
   is already staged, perform at most one actual network check per 24 hours.
4. Use an in-memory running flag so checks cannot overlap.
5. Log failures as [auto-update] daily check failed, without interrupting
   dictation or normal use.

Users can also check manually in Settings → About. The manual UI presents
checking, available, downloading, ready, up-to-date, and error states.

### Download, notification, and restart

When Tauri reports a newer version:

1. downloadUpdate() calls check(), then update.download(). It stages the
   update; it does not install it immediately.
2. The main webview retains the resulting Update handle in module scope as
   pendingUpdate.
3. The ready version is persisted in localStorage as
   airnote:auto-update:ready-version. If a secondary status-bar webview is
   recreated, AirNote can restore the restart reminder.
4. AirNote emits auto-update-ready. The status-bar shows:
   “Update downloaded — Restart AirNote to use it.”
5. Later hides the reminder for 24 hours but does not discard the staged
   update.
6. Restart emits airnote://apply-update to the main webview, where the update
   handle exists. applyPendingUpdate() calls install() then relaunch().
7. If the handle was lost through a webview reload, AirNote checks and
   downloads again before installing.
8. An apply failure emits airnote://apply-update-failed and preserves a retry
   message rather than silently failing.

On macOS, installation swaps the staged bundle at the apply/relaunch step. On
Windows, Tauri runs the NSIS installer, which closes the app itself.

## Versioning

The canonical version is [workspace.package].version in Cargo.toml. Do not
change only the Tauri configuration or artifact filename.

~~~bash
just bump 2.4.6
git diff -- Cargo.toml Cargo.lock crates/control-plane/Cargo.toml \
  desktop/package.json landing/package.json desktop/src-tauri/tauri.conf.json
~~~

scripts/bump-version.sh updates:

1. root Cargo workspace version
2. excluded control-plane crate version
3. desktop package version
4. landing package version
5. Tauri configuration version
6. Cargo.lock workspace version entries

The build script reads the root Cargo.toml version, so release filenames are
automatically derived as AirNote_<version>_aarch64.*.

Only create a release tag after the release commit has reached main:

~~~bash
just release 2.4.6
~~~

This creates tag v2.4.6 and pushes main plus tags. It does not build or upload
a DMG.

## Credentials: safe reuse versus product-specific keys

Secrets are deliberately not in this repository, .env, or this document.
scripts/release-credentials.sh loads them from local-only stores.

| Credential | Local storage convention | Use in another project |
| --- | --- | --- |
| Developer ID Application certificate | macOS login Keychain | Reusable for another app owned by the same Apple Developer team |
| Apple ID plus app-specific password, or a notarytool profile | macOS login Keychain | Reusable to notarize another app under the same team |
| Apple Team ID | release environment / loader default | Reusable within the same Apple Developer team |
| Tauri updater private key and password | ~/.tauri/said-updater.key (0600) plus Keychain item | **Do not reuse for another product** |

The updater private key is the installed product’s long-lived update trust
root. Reusing it would mean a compromise in one product could authorize updates
for another. Generate one update key per application.

For a new project, use its installed Tauri CLI to make a distinct key pair:

~~~bash
cd /path/to/new-project/desktop
npx tauri signer generate --ci -p "<store password in Keychain>" \
  -w "$HOME/.tauri/new-product-updater.key"
chmod 600 "$HOME/.tauri/new-product-updater.key"
~~~

Put the generated **public** key in that product’s updater configuration and
sign only that product’s archives with the corresponding **private** key. Give
the new project its own Keychain service names and private-key path.

AirNote’s production loader uses a Developer ID identity for EMIAC’s Apple
team, a Keychain-stored Apple notarization credential, a local updater private
key, and a Keychain-stored updater-key password. Do not copy their values into
another repository. The current machine can reuse its Apple certificate and
notary access, while the other project should make a new Tauri key.

### Important client-secret note

scripts/build-dmg.sh can read optional desktop cloud-provider values from .env
and bake selected values into the binary at compile time. That is an existing
AirNote product decision, not a signing requirement and not a pattern to copy
into a new project. Removing .env from the final app does not make a
compile-time-embedded credential secret. Prefer server-side credentials or
user-supplied keys in new products.

## Release-grade build: just dmg

Run on a Mac with Xcode/Command Line Tools, the Developer ID certificate, and
the required Keychain items:

~~~bash
just dmg
# or, where the machine/toolchain supports it:
just dmg x86_64-apple-darwin
~~~

just dmg calls scripts/build-dmg.sh. It performs a local build only, but it is
a distributable build: it preflights Apple notarization and Tauri updater
signing credentials before compiling.

Its exact stages are:

1. Read the target and root Cargo.toml version.
2. Load notarization credentials and the Tauri updater signing key from secure
   local storage. Fail early if either is absent.
3. Locate the Apple clang runtime needed by the Metal/whisper build and add it
   to Rust linker flags when found.
4. Detach stale AirNote volumes and Tauri temporary RW DMGs. This prevents
   bundle_dmg.sh failures left by an earlier build.
5. Build the airnote-backend sidecar in release mode and copy it into Tauri’s
   externalBin slot.
6. Build whisper-cli and copy it into its externalBin slot.
7. Run the Tauri production build with updater artifacts enabled.
8. Place whisper-cli and the Silero VAD model inside the completed app bundle.
9. Remove any packaged .env file.
10. Sign nested executables first with hardened runtime and
    desktop/src-tauri/AirNote.entitlements. Then sign AirNote.app with the
    Developer ID Application identity.
11. Verify codesign, the bundle identifier com.emiac.airnote.desktop, and the
    embedded sidecar.
12. Create a polished Finder DMG with create-dmg, with an hdiutil fallback that
    includes an /Applications shortcut.
13. Sign the DMG container itself, mount it, verify the application inside,
    and detach it.
14. Submit the DMG to Apple notarytool, wait for acceptance, staple the ticket,
    and run spctl Gatekeeper assessment.

Expected outputs:

~~~text
target/aarch64-apple-darwin/release/bundle/macos/AirNote.app
target/aarch64-apple-darwin/release/bundle/dmg/AirNote_<version>_aarch64.dmg
~~~

The terminal must show all of these before public distribution:

~~~text
status: Accepted
The staple and validate action worked!
source=Notarized Developer ID
~~~

## Test build: just local-dmg

~~~bash
just local-dmg
~~~

This invokes scripts/build-local-dmg.sh, which sets:

~~~text
AIRNOTE_LOCAL_TEST_DMG=1
AIRNOTE_REQUIRE_NOTARIZATION=1
NOTARY_TIMEOUT=45m
~~~

It still builds, signs, notarizes, staples, and Gatekeeper-validates a DMG.
The crucial difference: Tauri updater artifacts are disabled for this build.
It does not bump a version, commit source, push, upload to the VM, or modify
an update manifest.

Use this to QA the current checkout, replace a VM test-folder DMG, or hand a
tester a locally built installer. Never promote a just local-dmg result as an
in-app update; it has no Tauri updater archive/signature.

## Publishing an already-built Mac release

After just dmg succeeds and Gatekeeper validates the DMG:

~~~bash
./scripts/deploy-release-vm.sh aarch64-apple-darwin
~~~

This is intentionally a publisher, not a builder. It refuses to proceed unless
the app and notarized DMG already exist. Its work is:

1. Gatekeeper-assess the DMG locally, unless explicitly overridden.
2. Load the Tauri updater signing key from the secure credential loader.
3. Create AirNote_<version>_aarch64.app.tar.gz from the completed app.
   COPYFILE_DISABLE=1 and tar --no-xattrs prevent macOS ._* AppleDouble files;
   those files can make Tauri fail when unpacking an update.
4. Sign the archive using npx tauri signer sign, creating .tar.gz.sig.
5. Create a Darwin-specific latest.json containing the version, UTC date,
   darwin-aarch64 platform key, archive URL, and signature.
6. Generate SHA256SUMS for the DMG, archive, signature, and manifest.
7. Upload all versioned files and update the current Darwin manifest.
8. Retain the latest three release directories by default. Before pruning, keep
   any version still referenced by a Darwin, Windows, or legacy manifest.

The script accepts REMOTE, REMOTE_RELEASE_ROOT, PUBLIC_BASE_URL, and
KEEP_RELEASES overrides for another environment. Use SSH keys or a secure
secret injection mechanism; do not put a VM password in code or docs.

### Production VM structure

The verified release root is:

~~~text
/opt/airnote-control-plane/releases/
├── updates/
│   ├── darwin/latest.json
│   ├── windows/latest.json
│   └── latest.json                  # legacy combined fallback, not primary
├── releases/
│   └── <version>/
│       ├── AirNote_<version>_aarch64.dmg
│       ├── AirNote_<version>_aarch64.app.tar.gz
│       ├── AirNote_<version>_aarch64.app.tar.gz.sig
│       ├── darwin-latest.json
│       └── SHA256SUMS
└── landing/                          # marketing site, unrelated to updates
~~~

The production gateway Caddy reads this release root at
/srv/airnote-releases and routes it as:

~~~caddy
handle_path /updates/* {
    root * /srv/airnote-releases/updates
    header Cache-Control "no-store"
    file_server
}

handle_path /releases/* {
    root * /srv/airnote-releases/releases
    header Cache-Control "no-cache"
    file_server
}
~~~

The no-store policy on manifests prevents stale update decisions. The no-cache
policy on versioned artifacts permits revalidation without serving a stale
file. The public gateway configuration is /opt/gateway/Caddyfile; the
control-plane compose file also contains a read-only release mount for its
local Caddy service.

### Windows must stay separate

A Mac release writes only:

~~~text
updates/darwin/latest.json
releases/<version>/darwin-latest.json
~~~

It must never overwrite updates/windows/latest.json. Windows requires its own
installer, signature, and windows-x86_64 platform manifest. After every Mac
release, fetch the Windows manifest to make sure its version and URL are still
unchanged.

## Verification checklist

### Before building

~~~bash
git fetch origin
git status --short --branch
git diff --check
cargo fmt --all --check
cd desktop && npm run typecheck
~~~

Run just check before committing. If control-plane/admin code is included, also
run:

~~~bash
cd crates/control-plane && cargo check
cd crates/control-plane/admin-ui && pnpm run typecheck && pnpm run build
~~~

### Local release proof

~~~bash
just dmg

VERSION="$(awk '/^version = / { gsub(/.*"|".*/, ""); print; exit }' Cargo.toml)"
DMG="target/aarch64-apple-darwin/release/bundle/dmg/AirNote_${VERSION}_aarch64.dmg"
spctl --assess --type open --context context:primary-signature -v "$DMG"
~~~

Then manually open the DMG, install into a clean /Applications location,
launch it, verify required macOS permissions, and run dictation before
publishing.

### Public proof after publishing

~~~bash
./scripts/deploy-release-vm.sh aarch64-apple-darwin

curl -fsS https://airnote.emiactech.com/updates/darwin/latest.json
curl -fsSI "https://airnote.emiactech.com/releases/$VERSION/AirNote_${VERSION}_aarch64.dmg"
curl -fsSI "https://airnote.emiactech.com/releases/$VERSION/AirNote_${VERSION}_aarch64.app.tar.gz"
curl -fsS https://airnote.emiactech.com/updates/windows/latest.json
curl -fsS "https://airnote.emiactech.com/releases/$VERSION/SHA256SUMS"
~~~

Confirm:

- Darwin manifest version equals the released version.
- darwin-aarch64 URL matches the new updater archive.
- manual DMG and updater archive both return HTTP 200.
- SHA256SUMS contains the DMG, archive, signature, and manifest.
- Windows manifest remains its independent expected platform/version.
- an installed lower-version app shows Update downloaded, applies Restart, and
  launches at the new version.

## Safely test the updater without a production release

tools/update-harness/ uses a throwaway test minisign key, never the production
updater private key.

~~~bash
cd tools/update-harness
./gen-keys.sh
./publish.sh 99.0.0
./serve.sh
# another terminal
./smoke.sh
~~~

Phase 1 validates manifest shape, semantic version, and reachable artifact.
Phase 2 uses a development-only endpoint/public-key override to exercise the
real flow:

~~~text
check → download → authenticated install → update-ready notification → restart
~~~

Use a disposable app copy for an actual install/relaunch test. Never point a
production app at a test key or put the production updater private key in the
test harness.

## Common failures

| Symptom | Cause | Correct response |
| --- | --- | --- |
| bundle_dmg.sh fails or a volume is busy | old Tauri working image remains mounted | Re-run just dmg; its pre-clean stage detaches AirNote temporary volumes |
| Gatekeeper warns | DMG wasn't notarized/stapled | Stop; require Accepted, staple success, and source=Notarized Developer ID |
| updater does not see a release | cache, same/lower semantic version, wrong target, or manifest problem | Check Darwin manifest, cache header, darwin-aarch64 URL, and version |
| updater cannot install | wrong updater key/signature or AppleDouble files in archive | Re-publish with deploy-release-vm.sh; it suppresses ._* entries and signs the archive |
| Windows gets altered by Mac release | manifests were mixed | Restore the Windows manifest; Mac publisher must not touch it |
| Apple notarization returns 401 | missing/invalid Keychain credential or profile | Repair the local secure credential; never paste it into source or docs |

## Source of truth files

| File | Responsibility |
| --- | --- |
| justfile | public release commands |
| scripts/bump-version.sh | all pinned version edits |
| scripts/release-credentials.sh | secure local credential loading |
| scripts/build-dmg.sh | build, nested signing, DMG signing, notarization, stapling |
| scripts/build-local-dmg.sh | local/test notarized DMG with updater artifacts off |
| scripts/deploy-release-vm.sh | updater archive signing, manifest, checksums, upload, retention |
| desktop/src-tauri/tauri.conf.json | updater public key/endpoints and app bundle identity |
| desktop/src/lib/autoUpdate.ts | polling, staging, restart/apply coordination |
| desktop/src/StatusBar.tsx | ready notification and Later/Restart UX |
| tools/update-harness/ | throwaway-key updater test pipeline |

## Read-only VM diagnostics

These commands only inspect production state:

~~~bash
ssh root@103.180.163.41 \
  'sed -n "1,160p" /opt/airnote-control-plane/releases/updates/darwin/latest.json'

ssh root@103.180.163.41 \
  'find /opt/airnote-control-plane/releases/releases -maxdepth 2 -type f -printf "%p %s bytes\n" | sort'

ssh root@103.180.163.41 \
  'grep -n -A12 -B4 -E "airnote|/updates|/releases" /opt/gateway/Caddyfile'
~~~

Do release writes through scripts/deploy-release-vm.sh rather than ad-hoc file
copies. That is what preserves updater signing, checksums, platform separation,
and old-release retention.
