fn main() {
    // Re-link the crate whenever a baked option_env! key changes, so a cached
    // object file never ships a stale or missing key. Mirrors crates/backend/build.rs.
    //   DEEPSEEK_API_KEY  — meeting summaries (meeting_engine.rs)
    //   DEEPINFRA_API_KEY — DeepInfra cloud dictation STT (dictation_stt.rs)
    println!("cargo:rerun-if-env-changed=DEEPSEEK_API_KEY");
    println!("cargo:rerun-if-env-changed=DEEPINFRA_API_KEY");

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=13.0");
        for path in swift_runtime_search_paths() {
            println!("cargo:rustc-link-search=native={path}");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    ensure_dev_external_bin_placeholders();
    tauri_build::build()
}

fn ensure_dev_external_bin_placeholders() {
    if std::env::var("PROFILE").ok().as_deref() == Some("release") {
        return;
    }

    let Ok(target) = std::env::var("TARGET") else {
        return;
    };
    if target.is_empty() {
        return;
    }

    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let binaries_dir = std::path::PathBuf::from(manifest_dir).join("binaries");
    if std::fs::create_dir_all(&binaries_dir).is_err() {
        return;
    }

    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    for stem in ["airnote-backend", "whisper-cli"] {
        let path = binaries_dir.join(format!("{stem}-{target}{extension}"));
        if path.exists() {
            continue;
        }
        if std::fs::write(
            &path,
            placeholder_binary_contents(target.contains("windows")),
        )
        .is_ok()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

fn placeholder_binary_contents(is_windows: bool) -> &'static [u8] {
    if is_windows {
        b""
    } else {
        b"#!/bin/sh\necho 'AirNote development placeholder: build the real sidecar binary before runtime.' >&2\nexit 127\n"
    }
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
