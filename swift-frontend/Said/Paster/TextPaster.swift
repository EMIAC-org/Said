import AppKit
import os

enum TextPaster {
    private static let logger = RuntimeLogger(category: "paster")

    /// Type text directly into focused app using a single synthetic keyboard event
    /// for the entire token. Matches Rust paster's `type_text()` — one CGEvent pair
    /// per token regardless of length, with 6ms delays matching HID queue timing.
    /// Returns true if typed, false if Accessibility not granted.
    static func typeText(_ text: String) -> Bool {
        guard !text.isEmpty else { return true }
        guard PermissionHelper.accessibilityGranted else { return false }

        var utf16 = Array(text.utf16)
        let src = CGEventSource(stateID: .combinedSessionState)
        guard let keyDown = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: true),
              let keyUp = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: false) else {
            return false
        }
        keyDown.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: &utf16)
        keyUp.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: &utf16)
        keyDown.post(tap: .cghidEventTap)
        usleep(6000)
        keyUp.post(tap: .cghidEventTap)
        usleep(6000)
        return true
    }

    /// Clipboard paste via Cmd+V.
    static func paste(_ text: String) {
        guard !text.isEmpty else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)

        guard PermissionHelper.accessibilityGranted else {
            logger.info("accessibility not granted — text copied to clipboard (\(text.count) chars)")
            return
        }

        let src = CGEventSource(stateID: .combinedSessionState)
        if let vDown = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(9), keyDown: true),
           let vUp = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(9), keyDown: false) {
            vDown.flags = .maskCommand
            vUp.flags = .maskCommand
            vDown.post(tap: .cgAnnotatedSessionEventTap)
            usleep(6000)
            vUp.post(tap: .cgAnnotatedSessionEventTap)
        }
        logger.info("pasted \(text.count) chars via Cmd+V")
    }

    /// Read currently selected text from the focused app.
    /// Tries AX first, falls back to Cmd+C + clipboard read.
    static func readSelectedText() -> String? {
        let systemWide = AXUIElementCreateSystemWide()
        var focusedApp: AnyObject?
        AXUIElementCopyAttributeValue(systemWide, kAXFocusedApplicationAttribute as CFString, &focusedApp)
        if let app = focusedApp {
            var focusedElement: AnyObject?
            AXUIElementCopyAttributeValue(app as! AXUIElement, kAXFocusedUIElementAttribute as CFString, &focusedElement)
            if let element = focusedElement {
                var selectedText: AnyObject?
                let result = AXUIElementCopyAttributeValue(element as! AXUIElement, kAXSelectedTextAttribute as CFString, &selectedText)
                if result == .success, let text = selectedText as? String, !text.isEmpty {
                    return text
                }
            }
        }

        // Fallback: Cmd+C then read clipboard
        let pasteboard = NSPasteboard.general
        let original = pasteboard.string(forType: .string) ?? ""
        pasteboard.clearContents()

        let src = CGEventSource(stateID: .combinedSessionState)
        if let cDown = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(8), keyDown: true),
           let cUp = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(8), keyDown: false) {
            cDown.flags = .maskCommand
            cUp.flags = .maskCommand
            cDown.post(tap: .cgAnnotatedSessionEventTap)
            usleep(10_000)
            cUp.post(tap: .cgAnnotatedSessionEventTap)
        }
        usleep(100_000)

        let copied = pasteboard.string(forType: .string)
        pasteboard.clearContents()
        pasteboard.setString(original, forType: .string)
        return copied
    }

    /// Select-all (Cmd+A) then paste (Cmd+V) — replaces partial HID output.
    /// Matches Rust paster's `paste_replacing()` for safety paste when
    /// word-by-word typing partially failed.
    static func pasteReplacing(_ text: String) {
        guard !text.isEmpty else { return }
        guard PermissionHelper.accessibilityGranted else {
            paste(text)
            return
        }

        let pasteboard = NSPasteboard.general
        let original = pasteboard.string(forType: .string) ?? ""
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        usleep(80_000)

        let src = CGEventSource(stateID: .combinedSessionState)

        // Cmd+A
        if let aDown = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(0), keyDown: true),
           let aUp = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(0), keyDown: false) {
            aDown.flags = .maskCommand
            aUp.flags = .maskCommand
            aDown.post(tap: .cgAnnotatedSessionEventTap)
            usleep(10_000)
            aUp.post(tap: .cgAnnotatedSessionEventTap)
            usleep(20_000)
        }

        // Cmd+V
        if let vDown = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(9), keyDown: true),
           let vUp = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(9), keyDown: false) {
            vDown.flags = .maskCommand
            vUp.flags = .maskCommand
            vDown.post(tap: .cgAnnotatedSessionEventTap)
            usleep(10_000)
            vUp.post(tap: .cgAnnotatedSessionEventTap)
        }

        usleep(400_000)
        pasteboard.clearContents()
        pasteboard.setString(original, forType: .string)
        logger.info("paste-replacing \(text.count) chars — clipboard restored")
    }
}
