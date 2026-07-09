# Contributing to AirNote

Thanks for your interest. AirNote is a small project; the contribution flow is informal but a few conventions keep the codebase coherent.

---

## TL;DR for first-time contributors

### macOS

```bash
git clone https://github.com/EMIAC-org/Said
cd Said
cp .env.example .env       # fill in GATEWAY_API_KEY if using the gateway
brew install just          # if you don't have it
just dev                   # builds + launches Tauri desktop app
```

### Windows

`dev.sh` is bash-only; on Windows skip it and build the pieces directly:

```powershell
git clone https://github.com/EMIAC-org/Said
cd Said
Copy-Item .env.example .env   # fill in GATEWAY_API_KEY if using the gateway

# Install just via scoop / winget (one-time):
#   scoop install just
#   # or
#   winget install Casey.Just

# Build the backend sidecar and sync it into the Tauri externalBin slot:
cargo build -p said-backend
New-Item -ItemType Directory -Force -Path desktop\src-tauri\binaries | Out-Null
Copy-Item target\debug\airnote-backend.exe desktop\src-tauri\binaries\airnote-backend-x86_64-pc-windows-msvc.exe

# Then run the desktop dev loop:
cd desktop
npm ci
npm run tauri:dev
```

`just check` works on both platforms once `just` is installed.

### Sending a PR

```bash
just check                 # fmt-check + clippy + tests + typecheck — must pass
git checkout -b my-fix
git commit -m "..."
gh pr create
```

---

## What kinds of changes are welcome

- **Bug fixes** of any size.
- **Performance work** with before/after numbers (latency, memory, cold start).
- **Hinglish / multilingual** improvements — bring a recorded test case in the PR.
- **Cross-platform code** (the Windows port is in progress — see the roadmap in [AGENTS.md](AGENTS.md)).
- **Docs and copy fixes**.

What is *not* a good first PR:

- Large architectural rewrites without prior discussion in an issue.
- New external service integrations — open an issue first; we keep the dependency surface intentionally narrow.
- Style-only commits (we run `just fmt`).

---

## Project structure

See [AGENTS.md](AGENTS.md) for the full map. Briefly:

| Path | What lives there |
|---|---|
| `crates/hotkey` | Global hotkey listener (macOS today; Windows planned) |
| `crates/recorder` | Audio capture via `cpal` |
| `crates/core` | Shared transcript metadata + polish helpers |
| `crates/paster` | HID typing + edit-watch (macOS today; Windows planned) |
| `crates/backend` | Local Axum daemon — routes, LLM polish, SQLite, learning |
| `desktop/src-tauri` | Tauri shell (spawns `airnote-backend`) |
| `desktop/src` | React + Vite frontend |
| `scripts` | `build-dmg.sh`, `bump-version.sh` |
| `.github/workflows` | CI + release |

---

## Style and design rules (non-negotiable)

These are in [AGENTS.md](AGENTS.md) under "Design Rules" but worth repeating in PR review terms:

1. **`just check` must pass.** No exceptions; CI runs the same.
2. **HID delays in `paster/src/lib.rs` are sacred.** The 6 ms keydown→keyup pacing fixes word-merging at streaming speeds. Don't touch without an explanation in the PR description.
3. **Shipped sidecar binary name is `airnote-backend` everywhere** — crate/package name may remain `said-backend`, but packaged/runtime binaries must not ship as `said-backend`.
4. **`crates/control-plane` stays excluded from the workspace.** Build it standalone.
5. **STT transcript is NOT ground truth.** The classifier in `crates/backend/src/llm/classifier.rs` explicitly handles cases where STT and polish agree on the wrong word.
6. **Lexicon cache needs explicit invalidation.** Any route that writes to `corrections` or `stt_replacements` must call `invalidate_lexicon_cache()`.

---

## Commit conventions

- **Subject line**: imperative present-tense, under 70 chars. Examples: "Fix word merge on Slack", "Add Right-Alt fallback for Windows hotkey".
- **Body**: explain *why*, not *what*. The diff shows what.
- **Conventional commit tags** (`feat:`, `fix:`, `chore:`) are welcome but not required.
- **Sign-off**: not required.

Co-Authored-By trailers for AI assistants are fine when the assistant did meaningful work.

---

## PRs

- Target `main`. The repo does not use a long-lived `develop` branch.
- Keep PRs scoped — one logical change per PR. A bug fix is not a refactor.
- The PR description should include a test plan: how a reviewer reproduces the behavior locally.
- For UI changes, include a 5-second screen recording or a screenshot.
- For Hinglish / language behavior changes, include a sample utterance and the expected polished output.

CI runs `just check` against the PR. If it goes red, fix it before asking for review.

---

## Releases

Maintainers only. The flow is:

```bash
just bump 2.2.0            # cascades version into all source-of-truth files
git commit -am "Bump to 2.2.0"
just release 2.2.0         # tags + pushes — triggers .github/workflows/release.yml
```

The release workflow builds DMG (macOS) and signs the auto-updater bundle. See [scripts/build-dmg.sh](scripts/build-dmg.sh) for the Mac build steps and [.github/workflows/release.yml](.github/workflows/release.yml) for the publishing logic.

### Release channels

- **Stable**: clean semver tag (`v2.2.0`). Updates the `latest.json` manifest at `https://github.com/EMIAC-org/Said/releases/latest/download/latest.json`. Every shipped client auto-updates from this URL.
- **Beta**: tag with a `-beta`, `-rc`, or `-alpha` suffix (e.g. `v2.2.0-beta1`). Marked as **prerelease** on GitHub. Emits a `beta.json` manifest attached only to that prerelease, at `https://github.com/EMIAC-org/Said/releases/download/v2.2.0-beta1/beta.json`.

Until the in-app channel toggle ships (M1 follow-up), beta users must manually point their auto-updater at the specific beta tag URL. The infrastructure to make beta auto-discoverable (via a `manifests` branch or GitHub Pages) is tracked as a follow-up.

---

## Reporting bugs

- **Production bug**: file an issue with the app version (Settings → About), OS version, and what was on the clipboard when it broke (it's clipboard-based paste — the prior clipboard contents matter).
- **Privacy concern**: same place; flag with the `privacy` label.
- **Security**: email the maintainer privately rather than opening a public issue.

---

## License

By contributing, you agree your changes are MIT-licensed (see [LICENSE](LICENSE)).
