import SwiftUI

// ── Shared building blocks ───────────────────────────────────────────────────

struct KickerRow: View {
    let dot: Color
    let text: String
    var body: some View {
        HStack(spacing: 7) {
            Circle().fill(dot).frame(width: 7, height: 7)
            Text(text.uppercased())
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.3)
                .foregroundColor(Theme.inkDim)
        }
    }
}

struct ChipText: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .semibold))
            .padding(.horizontal, 7).padding(.vertical, 1)
            .background(Theme.accent)
            .foregroundColor(Color(white: 0.06))
            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

struct CardButton: View {
    enum Kind { case primary, ghost }
    let title: String
    var icon: String? = nil
    var kind: Kind = .ghost
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Text(title)
                if let icon { Image(systemName: icon).font(.system(size: 11, weight: .semibold)) }
            }
            .font(.system(size: 12, weight: .medium))
            .padding(.horizontal, 12).padding(.vertical, 7)
            .background(kind == .primary ? Theme.accent : Color.white.opacity(0.05))
            .foregroundColor(kind == .primary ? Color(white: 0.06) : Theme.ink)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(kind == .primary ? Color.clear : Theme.hairline, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

/// Standard panel chrome: kicker + body + trailing footer buttons.
struct CardShell<Body: View, Footer: View>: View {
    let dot: Color
    let kicker: String
    @ViewBuilder var content: () -> Body
    @ViewBuilder var footer: () -> Footer

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            KickerRow(dot: dot, text: kicker)
            content()
                .font(.system(size: 12.5))
                .foregroundColor(Theme.ink)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack(spacing: 8) { Spacer(); footer() }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

// ── Feedback cards ───────────────────────────────────────────────────────────

struct ConfirmCard: View {
    let term: String
    let original: String
    let recordingId: String
    let send: (OutboundAction) -> Void

    var body: some View {
        CardShell(dot: Theme.warn, kicker: "Quick question") {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 5) {
                    Text(original).strikethrough(color: Theme.inkFaint).foregroundColor(Theme.inkFaint)
                    Image(systemName: "arrow.right").font(.system(size: 9, weight: .semibold)).foregroundColor(Theme.inkFaint)
                    ChipText(text: term)
                }
                Text("Is “\(term)” a product, brand, or name?")
            }
        } footer: {
            CardButton(title: "No, just rephrasing", kind: .ghost) {
                send(OutboundAction(type: "confirm", decision: "skip", term: term, original: original, recordingId: recordingId))
            }
            CardButton(title: "Yes, learn it", icon: "return", kind: .primary) {
                send(OutboundAction(type: "confirm", decision: "learn", term: term, original: original, recordingId: recordingId))
            }
        }
    }
}

struct NegativeCard: View {
    let term: String
    let wrong: String
    let send: (OutboundAction) -> Void

    var body: some View {
        CardShell(dot: Theme.err, kicker: "Wrong correction detected") {
            VStack(alignment: .leading, spacing: 6) {
                (Text("AirNote keeps changing ")
                 + Text(term).foregroundColor(Theme.accent).bold()
                 + Text(" to “\(wrong)” but you changed it back."))
                Text("Should I stop this correction?")
            }
        } footer: {
            CardButton(title: "It was right", kind: .ghost) {
                send(OutboundAction(type: "dismiss"))
            }
            CardButton(title: "Yes, stop it", icon: "return", kind: .primary) {
                send(OutboundAction(type: "block", term: term, variant: term, wrongReplacement: wrong))
            }
        }
    }
}

struct ErrorCard: View {
    let message: String
    let runId: String?
    let audioId: String?
    let errorCode: String?
    let rawError: String?
    let diagnostic: String?
    let send: (OutboundAction) -> Void

    private var details: String {
        [
            message,
            errorCode.map { "code=\($0)" },
            runId.map { "run_id=\($0)" },
            audioId.map { "audio_id=\($0)" },
            diagnostic,
            rawError,
        ]
        .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
        .joined(separator: "\n")
    }

    var body: some View {
        CardShell(dot: Theme.err, kicker: "Couldn’t polish") {
            Text(message).foregroundColor(Theme.ink)
        } footer: {
            if let audioId {
                CardButton(title: "Retry", kind: .primary) { send(OutboundAction(type: "retry", audioId: audioId)) }
                CardButton(title: "Audio", kind: .ghost) { send(OutboundAction(type: "open_audio", audioId: audioId)) }
            }
            CardButton(title: "Copy", kind: .ghost) { send(OutboundAction(type: "copy_details", text: details)) }
            CardButton(title: "Dismiss", kind: .ghost) { send(OutboundAction(type: "dismiss")) }
        }
    }
}

struct UpdateCard: View {
    let version: String
    let message: String
    let send: (OutboundAction) -> Void

    var body: some View {
        CardShell(dot: Theme.ok, kicker: "Update downloaded") {
            Text(message)
        } footer: {
            CardButton(title: "Later", kind: .ghost) { send(OutboundAction(type: "snooze_update")) }
            CardButton(title: "Restart", kind: .primary) { send(OutboundAction(type: "apply_update")) }
        }
    }
}

// ── Review card (multi-correction, A/B/C, selectable rows) ──────────────────

struct ReviewCard: View {
    let recordingId: String
    let send: (OutboundAction) -> Void
    @State private var cands: [ReviewCandidate]

    init(candidates: [ReviewCandidate], recordingId: String, send: @escaping (OutboundAction) -> Void) {
        self.recordingId = recordingId
        self.send = send
        _cands = State(initialValue: candidates)
    }

    private var selectedCount: Int { cands.filter { $0.selected }.count }
    private let letters = ["A", "B", "C", "D", "E", "F"]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("\(selectedCount) selected" + (selectedCount != cands.count ? " · \(cands.count) total" : ""))
                    .font(.system(size: 11, weight: .semibold)).foregroundColor(Theme.inkDim)
                Spacer()
            }
            Text("These will be learned").font(.system(size: 11)).foregroundColor(Theme.inkFaint)

            VStack(spacing: 5) {
                ForEach(Array(cands.enumerated()), id: \.element.id) { idx, c in
                    Button { cands[idx].selected.toggle() } label: {
                        HStack(alignment: .top, spacing: 9) {
                            Text(letters[min(idx, letters.count - 1)])
                                .font(.system(size: 10, weight: .bold))
                                .frame(width: 18, height: 18)
                                .background(c.selected ? Theme.accent : Color.white.opacity(0.08))
                                .foregroundColor(c.selected ? Color(white: 0.06) : Theme.inkDim)
                                .clipShape(RoundedRectangle(cornerRadius: 5))
                            (Text(c.corrected).foregroundColor(Theme.ink).font(.system(size: 12, weight: .semibold))
                             + Text("  — was “\(c.original.isEmpty ? "—" : c.original)” · \(c.tag)")
                                .foregroundColor(Theme.inkFaint).font(.system(size: 12)))
                                .fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 9).padding(.vertical, 7)
                        .background(c.selected ? Theme.accentSoft : Color.white.opacity(0.03))
                        .overlay(RoundedRectangle(cornerRadius: 9).stroke(c.selected ? Theme.accent.opacity(0.35) : .clear, lineWidth: 1))
                        .clipShape(RoundedRectangle(cornerRadius: 9))
                    }
                    .buttonStyle(.plain)
                }
            }

            Spacer(minLength: 0)
            HStack(spacing: 8) {
                Spacer()
                CardButton(title: "Skip", kind: .ghost) { send(OutboundAction(type: "dismiss")) }
                CardButton(title: selectedCount > 0 ? "Learn \(selectedCount)" : "Learn", icon: "return", kind: .primary) {
                    let items = cands.filter { $0.selected }.map { BatchItem(original: $0.original, corrected: $0.corrected) }
                    guard !items.isEmpty else { return }
                    send(OutboundAction(type: "confirm_batch", recordingId: recordingId, items: items))
                }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

// ── Learning toasts ──────────────────────────────────────────────────────────

struct ToastRow<Trailing: View>: View {
    let dot: Color?
    let spinner: Bool
    let content: Text
    @ViewBuilder var trailing: () -> Trailing

    init(dot: Color? = nil, spinner: Bool = false, content: Text, @ViewBuilder trailing: @escaping () -> Trailing = { EmptyView() }) {
        self.dot = dot; self.spinner = spinner; self.content = content; self.trailing = trailing
    }

    var body: some View {
        HStack(spacing: 9) {
            if spinner { ProgressView().controlSize(.small).scaleEffect(0.7) }
            else if let dot { Circle().fill(dot).frame(width: 7, height: 7) }
            content.font(.system(size: 13)).foregroundColor(Theme.ink)
            Spacer(minLength: 0)
            trailing()
        }
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }
}

// ── Recents (hover-open) ─────────────────────────────────────────────────────

struct RecentsList: View {
    let items: [RecentItem]
    let send: (OutboundAction) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(items) { item in
                Button { send(OutboundAction(type: "copy_recent", text: item.text)) } label: {
                    HStack(spacing: 10) {
                        Text(item.text).font(.system(size: 12.5)).foregroundColor(Theme.ink)
                            .lineLimit(1).truncationMode(.tail)
                        Spacer(minLength: 6)
                        Text(item.ago).font(.system(size: 10.5)).foregroundColor(Theme.inkFaint)
                    }
                    .padding(.horizontal, 9).padding(.vertical, 7)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
