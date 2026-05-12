fn main() {
    // `#[cfg(target_os)]` inside build.rs is evaluated against the *host*
    // OS — that's wrong for cross-compile (host=macOS, target=windows
    // would still emit the framework link). Read CARGO_CFG_TARGET_OS to
    // pick up the actual build target.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
