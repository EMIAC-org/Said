//! Build script — re-bake the build-time keys captured via `option_env!`
//! whenever their values change. Without this, cargo would reuse a cached
//! object file with a stale (or missing) bundled key.
//!
//! These keys (Deepgram STT, Gateway/LLM) are baked into the shipped binary so
//! end users never enter API keys. `build-dmg.sh` exports them from `.env` at
//! build time; `.env` is gitignored, so the values are never committed.

fn main() {
    println!("cargo:rerun-if-env-changed=DEEPGRAM_API_KEY");
    println!("cargo:rerun-if-env-changed=GATEWAY_API_KEY");
}
