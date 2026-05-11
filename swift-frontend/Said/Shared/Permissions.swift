import AVFoundation
import ApplicationServices
import Cocoa

enum PermissionHelper {
    static var microphoneGranted: Bool {
        AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
    }

    static func requestMicrophone() async -> Bool {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        if status == .authorized { return true }
        return await AVCaptureDevice.requestAccess(for: .audio)
    }

    static var accessibilityGranted: Bool {
        AXIsProcessTrusted()
    }

    static func requestAccessibility() {
        let options = [kAXTrustedCheckOptionPrompt.takeRetainedValue(): true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
        openPrivacyPane("Privacy_Accessibility")
    }

    static var inputMonitoringGranted: Bool {
        CGPreflightListenEventAccess()
    }

    static func requestInputMonitoring() {
        if !CGPreflightListenEventAccess() {
            CGRequestListenEventAccess()
        }
        openPrivacyPane("Privacy_ListenEvent")
    }

    static func openPrivacyPane(_ anchor: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)") {
            NSWorkspace.shared.open(url)
        }
    }
}
