import Carbon
import Cocoa
import os

final class HotkeyManager: ObservableObject {
    @Published var isHeld = false
    @Published var activeKey: RecordHotkey = .capsLock

    var onPress: (() -> Void)?
    var onRelease: (() -> Void)?
    var onShortcut: ((UInt8) -> Void)?
    var onPasteLatest: (() -> Void)?

    private var tapThread: Thread?
    private var eventTap: CFMachPort?
    private let logger = RuntimeLogger(category: "hotkey")

    func start() {
        guard eventTap == nil else { return }
        if let tapThread, !tapThread.isFinished {
            return
        }
        let thread = Thread { [weak self] in
            self?.runTap()
        }
        thread.name = "said-hotkey"
        thread.qualityOfService = .userInteractive
        thread.start()
        tapThread = thread
    }

    private func runTap() {
        let mask: CGEventMask = (1 << CGEventType.flagsChanged.rawValue)
            | (1 << CGEventType.keyDown.rawValue)

        let callback: CGEventTapCallBack = { _, _, event, userInfo in
            guard let userInfo else { return Unmanaged.passRetained(event) }
            let mgr = Unmanaged<HotkeyManager>.fromOpaque(userInfo).takeUnretainedValue()

            if event.type == .flagsChanged {
                mgr.handleFlags(event)
                return Unmanaged.passRetained(event)
            }

            if event.type == .keyDown {
                let consumed = mgr.handleKeyDown(event)
                if consumed {
                    return nil
                }
            }

            return Unmanaged.passRetained(event)
        }

        let selfPtr = Unmanaged.passUnretained(self).toOpaque()
        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: mask,
            callback: callback,
            userInfo: selfPtr
        ) else {
            logger.error("CGEventTapCreate failed — Input Monitoring permission required")
            DispatchQueue.main.async { [weak self] in
                self?.tapThread = nil
                self?.eventTap = nil
            }
            return
        }

        eventTap = tap
        logger.info("CGEventTap active — listening for \(self.activeKey.rawValue) + shortcuts")

        let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
        CFRunLoopRun()
    }

    private func handleFlags(_ event: CGEvent) {
        let keycode = event.getIntegerValueField(.keyboardEventKeycode)
        let flags = event.flags

        switch activeKey {
        case .capsLock:
            guard keycode == 57 else { return }
            let down = flags.contains(.maskAlphaShift)
            updateHeld(down)

        case .fn:
            guard keycode == 63 else { return }
            let down = flags.contains(.maskSecondaryFn)
            updateHeld(down)

        case .rightOption:
            guard keycode == 61 else { return }
            let down = flags.rawValue & 0x0000_0040 != 0
            updateHeld(down)
        }
    }

    /// Returns true if the event was consumed (should be suppressed).
    private func handleKeyDown(_ event: CGEvent) -> Bool {
        let keycode = event.getIntegerValueField(.keyboardEventKeycode)
        let flags = event.flags

        let hasOption = flags.contains(.maskAlternate)
        let hasCmd = flags.contains(.maskCommand)
        let hasCtrl = flags.contains(.maskControl)

        // Option+1..5 — keycodes: 1=18, 2=19, 3=20, 4=21, 5=23
        if hasOption && !hasCmd && !hasCtrl {
            let n: UInt8? = switch keycode {
            case 18: 1
            case 19: 2
            case 20: 3
            case 21: 4
            case 23: 5
            default: nil
            }
            if let n {
                logger.info("Option+\(n) shortcut — consuming event")
                onShortcut?(n)
                return true
            }
        }

        // Ctrl+Cmd+V (keycode 9 = V)
        if hasCtrl && hasCmd && keycode == 9 {
            logger.info("Ctrl+Cmd+V — consuming event")
            onPasteLatest?()
            return true
        }

        return false
    }

    private func updateHeld(_ down: Bool) {
        if down && !isHeld {
            logger.info("\(self.activeKey.rawValue) held → start")
            DispatchQueue.main.async {
                self.isHeld = true
                self.onPress?()
            }
        } else if !down && isHeld {
            logger.info("\(self.activeKey.rawValue) released → stop")
            DispatchQueue.main.async {
                self.isHeld = false
                self.onRelease?()
            }
        }
    }
}
