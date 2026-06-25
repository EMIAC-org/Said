import SwiftUI

/// The chin outline: notch shape on notched Macs, rounded pill on flat displays.
struct ChinShape: Shape {
    var top: CGFloat
    var bottom: CGFloat
    let notch: Bool

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { .init(top, bottom) }
        set { top = newValue.first; bottom = newValue.second }
    }

    func path(in rect: CGRect) -> Path {
        if notch {
            return NotchShape(topCornerRadius: top, bottomCornerRadius: bottom).path(in: rect)
        }
        return RoundedRectangle(cornerRadius: bottom, style: .continuous).path(in: rect)
    }
}

/// A small recording-indicator dot that optionally pulses.
struct PulseDot: View {
    let color: Color
    let pulse: Bool
    @State private var on = false

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 9, height: 9)
            .shadow(color: color.opacity(0.55), radius: 4)
            .scaleEffect(pulse && on ? 1.25 : 1.0)
            .opacity(pulse && on ? 0.5 : 1.0)
            .onAppear {
                guard pulse else { return }
                withAnimation(.easeInOut(duration: 0.65).repeatForever(autoreverses: true)) { on = true }
            }
    }
}

/// Root HUD view. Draws the black notch shape sized to `model.box`, top-centred
/// inside the (often larger) fixed window. Growing the *shape* — not the window
/// — is what makes the notch expand smoothly with no black pop.
struct NotchView: View {
    @ObservedObject var model: HUDModel
    let hasNotch: Bool
    let closedHeight: CGFloat
    let send: (OutboundAction) -> Void

    private var box: CGSize {
        model.box == .zero ? CGSize(width: 200, height: closedHeight) : model.box
    }
    private var isOpen: Bool { model.state.isOpen }
    private var topInset: CGFloat { hasNotch ? closedHeight : 8 }

    var body: some View {
        content
            .padding(.top, isOpen ? topInset : 0)
            .frame(width: box.width, height: box.height, alignment: .top)
            .background(Color.black)
            .clipShape(clip)
            .overlay(alignment: .top) {
                if hasNotch {
                    Circle()
                        .fill(RadialGradient(colors: [Color(white: 0.16), .black],
                                             center: .topLeading, startRadius: 0, endRadius: 6))
                        .frame(width: 8, height: 8)
                        .padding(.top, max((closedHeight - 8) / 2, 5))
                }
            }
            .overlay(alignment: .topLeading) { recDot }
            // top-centre the shape inside the (larger) fixed window
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private var clip: ChinShape {
        ChinShape(
            top: isOpen ? Theme.topRadiusOpen : Theme.topRadiusClosed,
            bottom: isOpen ? Theme.bottomRadiusOpen : (hasNotch ? Theme.bottomRadiusClosed : 17),
            notch: hasNotch
        )
    }

    // amber/periwinkle/green recording dot, in the notch bar (top-left of the shape)
    @ViewBuilder private var recDot: some View {
        Group {
            switch model.state {
            case .listening: PulseDot(color: Theme.amber, pulse: true)
            case .polishing: PulseDot(color: Theme.accent, pulse: true)
            case .pasted:    PulseDot(color: Theme.ok, pulse: false)
            default:         EmptyView()
            }
        }
        .padding(.top, 11).padding(.leading, 17)
    }

    @ViewBuilder private var content: some View {
        switch model.state {
        case .idle:
            Color.clear

        // ── minimal voice: live transcript only, rolling (latest words at right) ──
        case .listening:
            transcriptChin(Theme.inkDim)
        case .polishing:
            transcriptChin(Theme.inkDim)
        case .pasted(let text):
            pastedChin(text)

        // ── learning toasts ──
        case .manualPaste(let message):
            ToastRow(dot: Theme.warn, content: boldLead("⌘V", " \(message)"))
        case .learned(let term, _):
            ToastRow(dot: Theme.ok, content: boldLead(term, " learned")) {
                Button { send(OutboundAction(type: "undo", term: term)) } label: {
                    Label("Undo", systemImage: "arrow.uturn.backward")
                        .font(.system(size: 12, weight: .semibold)).foregroundColor(Theme.accent)
                }.buttonStyle(.plain)
            }
        case .emailSaved(_, let message):
            ToastRow(dot: Theme.ok, content: Text(message))
        case .queued(let term, let remaining):
            ToastRow(dot: Theme.accent,
                     content: boldLead(term, " — \(remaining) more edit\(remaining == 1 ? "" : "s") to learn"))
        case .wrongFixed(let term, let wrong):
            ToastRow(dot: Theme.ok, content: Text("Got it — won’t type “\(wrong)” for “\(term)”"))
        case .retraining:
            ToastRow(spinner: true, content: Text("Improving model…"))
        case .retrainDone(let s):
            ToastRow(dot: Theme.ok, content: Text(s > 0 ? "Model updated (\(String(format: "%.1f", s))s)" : "Model updated"))

        // ── feedback cards ──
        case .confirming(let term, let original, let recordingId):
            ConfirmCard(term: term, original: original, recordingId: recordingId, send: send)
        case .negativeConfirm(let term, let wrong):
            NegativeCard(term: term, wrong: wrong, send: send)
        case .reviewing(let cands, let recordingId):
            ReviewCard(candidates: cands, recordingId: recordingId, send: send).id(recordingId)

        // ── system ──
        case .error(let message, let audioId):
            ErrorCard(message: message, audioId: audioId, send: send)
        case .updateReady(let version, let message):
            UpdateCard(version: version, message: message, send: send)
        case .recents(let items):
            RecentsList(items: items, send: send)
        case .placement(let message):
            ToastRow(dot: Theme.accent, content: Text(message))
        }
    }

    /// One-line live transcript that rolls: the tail (latest words) stays visible,
    /// older words truncate on the left under a soft fade.
    private func transcriptChin(_ color: Color) -> some View {
        Text(model.liveTranscript.isEmpty ? "…" : model.liveTranscript)
            .font(.system(size: 12.5))
            .foregroundColor(color)
            .lineLimit(1)
            .truncationMode(.head)
            .frame(maxWidth: .infinity, alignment: .trailing)
            .mask(LinearGradient(
                stops: [.init(color: .clear, location: 0),
                        .init(color: .black, location: 0.14),
                        .init(color: .black, location: 1)],
                startPoint: .leading, endPoint: .trailing))
            .padding(.horizontal, 14).padding(.bottom, 6)
    }

    private func pastedChin(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12.5))
            .foregroundColor(Theme.ink)
            .lineLimit(2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.bottom, 6)
    }

    private func boldLead(_ lead: String, _ rest: String) -> Text {
        Text(lead).fontWeight(.semibold).foregroundColor(Theme.ink)
            + Text(rest).foregroundColor(Theme.inkDim)
    }
}
