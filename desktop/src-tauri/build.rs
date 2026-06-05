fn main() {
    #[cfg(target_os = "macos")]
    {
        // The `system-echo-gate` feature links a Swift bridge + ScreenCaptureKit
        // and MetalFX, which only exist on macOS 13+. Without it the app has no
        // framework requiring >11, so the default build targets macOS 11.0 and
        // reaches a far wider fleet. Cargo exposes enabled features to build
        // scripts as CARGO_FEATURE_<NAME>.
        if std::env::var_os("CARGO_FEATURE_SYSTEM_ECHO_GATE").is_some() {
            println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=13.0");
            for path in swift_runtime_search_paths() {
                println!("cargo:rustc-link-search=native={path}");
            }
            println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        } else {
            println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=11.0");
        }
    }
    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn swift_runtime_search_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(output) = std::process::Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
    {
        if output.status.success() {
            if let Ok(swiftc) = String::from_utf8(output.stdout) {
                let swiftc = std::path::PathBuf::from(swiftc.trim());
                if let Some(toolchain_usr) = swiftc.parent().and_then(|p| p.parent()) {
                    paths.push(toolchain_usr.join("lib/swift/macosx").display().to_string());
                }
            }
        }
    }
    paths.push("/Library/Developer/CommandLineTools/usr/lib/swift/macosx".into());
    paths.sort();
    paths.dedup();
    paths
}
