//! Build script — re-bake the build-time keys captured via `option_env!`
//! whenever their values change. Without this, cargo would reuse a cached
//! object file with a stale (or missing) bundled key.
//!
//! The gateway key can be baked into the shipped binary so end users never
//! enter API keys. `build-dmg.sh` exports it from `.env` at build time; `.env`
//! is gitignored, so the value is never committed.

fn main() {
    println!("cargo:rerun-if-env-changed=GATEWAY_API_KEY");
}
