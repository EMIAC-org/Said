import AppKit
import SwiftUI

/// Owns the panel, geometry, model, and bridge.
///
/// Window strategy (this is what kills the "black pop"): passive states share a
/// single fixed-size **stage** window; only the black notch *shape* inside it
/// animates, so the notch grows smoothly and is always black. Interactive card
/// states resize the window to fit (so the transparent margins don't eat clicks).
final class NotchController: NSObject, NSApplicationDelegate {
    private let model = HUDModel()
    private var metrics = NotchGeometry.current()
    private var panel: NotchPanel!
    private var hosting: NSHostingView<NotchView>!
    private var bridge: Bridge!
    private var generation = 0   // guards stale auto-hide timers

    // ── Lifecycle ────────────────────────────────────────────────────────────

    func applicationDidFinishLaunching(_ notification: Notification) {
        metrics = NotchGeometry.current()
        model.box = metrics.closedSize

        panel = NotchPanel(contentRect: stageFrame())
        hosting = NSHostingView(rootView: makeView())
        hosting.wantsLayer = true
        hosting.layer?.backgroundColor = .clear
        panel.contentView = hosting

        if ProcessInfo.processInfo.environment["AIRNOTE_NOTCH_DEBUG"] != nil {
            let f = metrics.screen.frame
            FileHandle.standardError.write(Data(
                "[notch] screen=\(f) hasNotch=\(metrics.hasNotch) closed=\(metrics.closedSize) stage=\(stageSize)\n".utf8))
        }

        applyState(.idle, animated: false)
        bridge = Bridge(onMessage: { [weak self] msg in self?.apply(msg) })
        bridge.start()
        bridge.sendReady()

        NotificationCenter.default.addObserver(
            self, selector: #selector(screensChanged),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)
    }

    private func makeView() -> NotchView {
        NotchView(model: model, hasNotch: metrics.hasNotch,
                  closedHeight: metrics.closedSize.height,
                  send: { [weak self] in self?.send($0) })
    }

    @objc private func screensChanged() {
        metrics = NotchGeometry.current()
        hosting.rootView = makeView()
        applyState(model.state, animated: false)
    }

    // The fixed stage window — big enough for any passive shape to animate in.
    private var stageSize: CGSize {
        let cw = metrics.closedSize.width
        let inset = metrics.hasNotch ? metrics.closedSize.height : 8
        return CGSize(width: max(420, cw * 1.5), height: inset + 58)
    }
    private var topGap: CGFloat { metrics.hasNotch ? 0 : 10 }
    private func stageFrame() -> NSRect {
        NotchGeometry.frame(for: stageSize, on: metrics.screen, topGap: topGap)
    }

    // ── Inbound: message → state ───────────────────────────────────────────────

    private func apply(_ m: InboundMessage) {
        switch m.type {
        case "state":
            switch m.kind {
            case "recording":
                if case .listening = model.state {} else {
                    model.liveTranscript = ""
                    setState(.listening(startedAt: Date()))
                }
            case "processing":
                if !model.state.isOpen || isVoice(model.state) {
                    setState(.polishing(phase: m.phase ?? "polishing"))
                }
            case "idle":
                setState(.idle)
            default: break
            }

        case "status":
            if let t = m.transcript { model.liveTranscript = t }
            if case .polishing = model.state, let p = m.phase {
                setState(.polishing(phase: p))
            } else if !model.state.isOpen, m.transcript != nil {
                setState(.polishing(phase: m.phase ?? "polishing"))
            }

        case "transcript":
            model.liveTranscript = m.text ?? ""

        case "level":
            // Voice meter updates are high-frequency. The current notch UI does
            // not render them, but Rust still sends them with the shared HUD
            // event stream; accept quietly so stderr stays useful.
            break

        case "done":
            setState(.pasted(text: model.liveTranscript))

        case "output":
            if m.status == "manual_paste" {
                setState(.manualPaste(message: m.message ?? "Press ⌘V to paste"))
            } else {
                setState(.pasted(text: m.message ?? model.liveTranscript))
            }

        case "error":
            setState(.error(message: m.message ?? "Something went wrong", audioId: m.audioId))
            if let ms = m.autoHideMs { scheduleHide(after: ms / 1000.0) }

        case "learned":
            setState(.learned(term: m.term ?? "", message: m.message ?? "Learned"))
        case "email_saved":
            setState(.emailSaved(email: m.email ?? "", message: m.message ?? "Email saved"))
        case "queued":
            setState(.queued(term: m.term ?? "", remaining: m.remaining ?? 1))
        case "wrong_fixed":
            setState(.wrongFixed(term: m.term ?? "", wrong: m.wrongReplacement ?? ""))
        case "retraining":
            setState(.retraining)
        case "retrain_done":
            setState(.retrainDone(durationS: m.durationS ?? 0))

        case "confirm":
            setState(.confirming(term: m.term ?? "", original: m.original ?? "", recordingId: m.recordingId ?? ""))
        case "negative_confirm":
            setState(.negativeConfirm(term: m.term ?? "", wrong: m.wrongReplacement ?? ""))
        case "review":
            let cands = (m.candidates ?? []).map {
                ReviewCandidate(original: $0.original, corrected: $0.corrected,
                                tag: $0.tag ?? "edit", learnable: $0.learnable ?? true,
                                selected: $0.learnable ?? true)
            }
            setState(.reviewing(candidates: cands, recordingId: m.recordingId ?? ""))

        case "update_ready":
            setState(.updateReady(version: m.version ?? "", message: m.message ?? "Restart to finish updating."))
        case "placement":
            setState(.placement(message: m.message ?? "Drag to reposition"))
        case "recents":
            let items = (m.recents ?? []).map { RecentItem(text: $0.text, ago: $0.ago ?? "") }
            setState(.recents(items))

        case "present":
            panel.orderFrontRegardless()
        case "dismiss":
            setState(.idle)

        default:
            bridge.log("unknown message type: \(m.type)")
        }
    }

    private func isVoice(_ s: HUDState) -> Bool {
        switch s { case .listening, .polishing, .pasted: return true; default: return false }
    }

    // ── State application ──────────────────────────────────────────────────────

    private func setState(_ s: HUDState) {
        generation += 1
        let myGen = generation
        applyState(s, animated: true)
        if let delay = s.autoHide {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                guard let self, self.generation == myGen else { return }
                self.setState(.idle)
            }
        }
    }

    private func scheduleHide(after delay: TimeInterval) {
        let myGen = generation
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, self.generation == myGen else { return }
            self.setState(.idle)
        }
    }

    private func applyState(_ s: HUDState, animated: Bool) {
        let box = boxSize(for: s)
        let interactive = s.isInteractive
        panel.setInteractive(interactive)

        // Window: stage for passive (so the common voice flow never resizes the
        // window → no pop), tight box for interactive cards.
        let targetWindow = interactive ? box : stageSize
        if panel.frame.size != targetWindow {
            panel.setFrame(
                NotchGeometry.frame(for: targetWindow, on: metrics.screen, topGap: topGap),
                display: true)
        }

        // Animate the SHAPE (box) — but only for passive states, where the window
        // stays put. Interactive cards appear at their final size (no clipping).
        if animated && !interactive {
            withAnimation(s.isOpen ? Theme.spring : Theme.springClose) {
                model.box = box
                model.state = s
            }
        } else {
            model.box = box
            model.state = s
        }

        applyVisibility(for: s)
    }

    /// On notched Macs the closed notch stays drawn (it overlays the real cutout).
    /// On flat displays the synthetic pill hides at idle, so it "appears" on use.
    private func applyVisibility(for s: HUDState) {
        if case .idle = s, !metrics.hasNotch {
            panel.orderOut(nil)
        } else {
            panel.orderFrontRegardless()
        }
    }

    /// Size of the black notch shape per state. Width never shrinks below the
    /// closed notch; height adds the chin below the camera zone (`ch`).
    private func boxSize(for s: HUDState) -> CGSize {
        let cw = metrics.closedSize.width
        let inset = metrics.hasNotch ? metrics.closedSize.height : 8
        func sz(_ w: CGFloat, _ body: CGFloat) -> CGSize {
            CGSize(width: max(w, cw), height: inset + body)
        }
        switch s {
        case .idle:                                   return metrics.closedSize
        // minimal voice: ~15% wider each side than the notch + one rolling line
        case .listening, .polishing:                  return CGSize(width: cw * 1.3, height: inset + 26)
        case .pasted:                                 return CGSize(width: cw * 1.44, height: inset + 40)
        case .manualPaste:                            return sz(300, 50)
        case .learned:                                return sz(300, 40)
        case .emailSaved:                             return sz(320, 40)
        case .queued:                                 return sz(330, 40)
        case .wrongFixed:                             return sz(350, 40)
        case .retraining, .retrainDone:               return sz(260, 40)
        case .confirming:                             return sz(330, 116)
        case .negativeConfirm:                        return sz(344, 126)
        case .reviewing(let c, _):                    return sz(388, CGFloat(60 + min(c.count, 5) * 44 + 28))
        case .error:                                  return sz(344, 80)
        case .updateReady:                            return sz(330, 96)
        case .recents(let items):                     return sz(440, CGFloat(16 + min(items.count, 5) * 34 + 8))
        case .placement:                              return sz(260, 34)
        }
    }

    // ── Outbound: user action → Rust, with optimistic local collapse ──────────

    private func send(_ action: OutboundAction) {
        bridge.send(action)
        switch action.type {
        case "dismiss", "block", "confirm", "confirm_batch", "snooze_update":
            setState(.idle)
        default:
            break
        }
    }
}
