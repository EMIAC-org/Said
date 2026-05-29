fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=13.0");
        for path in swift_runtime_search_paths() {
            println!("cargo:rustc-link-search=native={path}");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
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
