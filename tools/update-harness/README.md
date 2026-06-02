# Auto-update test harness

Exercises AirNote's **real** updater pipeline against a local mock server, so you
can validate the full check → download → verify → install → relaunch flow without
cutting a real release. Uses a throwaway TEST signing key — never the production key.

## Phase 1 — publish / sign / serve / validate (no app build needed)

```bash
cd tools/update-harness
./gen-keys.sh                 # one-time: throwaway minisign keypair → keys/
./publish.sh 99.0.0           # sign a (placeholder) bundle + write mock/latest.json
./serve.sh                    # serve mock/ on http://localhost:3007
# in another shell:
./smoke.sh                    # the CI promotion gate (run this before promoting ANY real manifest)
```

`smoke.sh` is the gate to wire into CI before promoting a manifest to stable. It
catches the worst prod failure: Tauri rejects the **whole** manifest if any one
platform entry is malformed, silently blocking updates for everyone.

What Phase 1 proves: the minisign signing toolchain works, the manifest is shaped
exactly as the updater expects, the server serves it, and the validation gate
accepts good manifests / rejects broken ones (missing url, bad semver, unreachable
artifact). The *signature* itself is verified by the app at install time → Phase 2.

## Phase 2 — in-app check → download → verify → pill (needs one rebuild)

Requires a dev-only override so the app points its updater at localhost with the
TEST pubkey. Add to the `tauri_plugin_updater::Builder` registration in
`desktop/src-tauri/src/main.rs` (guarded by env vars so it never affects release):

```rust
let mut b = tauri_plugin_updater::Builder::new();
if let Ok(ep) = std::env::var("AIRNOTE_UPDATE_ENDPOINT") {
    b = b.endpoints(vec![ep]).expect("override endpoint");
    if let Ok(pk) = std::env::var("AIRNOTE_UPDATE_PUBKEY") { b = b.pubkey(pk); }
}
.plugin(b.build())
```

Then run the installed/dev app with a low version against the mock:

```bash
export AIRNOTE_UPDATE_ENDPOINT="http://localhost:3007/latest.json"
export AIRNOTE_UPDATE_PUBKEY="$(cat tools/update-harness/keys/test.key.pub)"
./dev.sh
```

The app's `autoUpdate.ts` will find 99.0.0 > current, download from localhost,
verify the signature against the test pubkey, and raise the "Update ready · Restart"
pill in the status bar. Use a throwaway copy of the .app when testing the actual
destructive install+relaunch swap.

## Failure modes to exercise (Phase 2)
- corrupt signature → flip a char in `mock/latest.json` `signature` → install must reject after download
- network down → stop `serve.sh` → `check()` throws, app must stay alive
- endpoint fallback → set two endpoints, first a dead port → advances only on non-2XX
- forced gate → add `"minimum_version"` to the manifest, run an older app → UI blocks
