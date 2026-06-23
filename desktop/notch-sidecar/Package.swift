// swift-tools-version:5.9
import PackageDescription

// AirNote Notch HUD — native macOS sidecar.
//
// A standalone executable spawned by the Tauri shell. It renders the AirNote
// status-bar pill state machine in/around the MacBook notch (boring.notch-style
// dynamic island), and talks to Rust over stdin/stdout (newline-delimited JSON).
//
// Build:   swift build -c release    (binary: .build/release/AirNoteNotch)
// Ship:    copied to desktop/src-tauri/binaries/airnote-notch-<triple> by `just`.
let package = Package(
    name: "AirNoteNotch",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "AirNoteNotch",
            path: "Sources/AirNoteNotch"
        )
    ]
)
