import SwiftUI

// MARK: - Tone presets (mirror the desktop's server-backed tone vocabulary)

struct AirNoteTone: Identifiable {
    var key: String
    var label: String
    var detail: String
    var id: String { key }

    /// The exact tone keys the server + desktop use. The server's DB default is
    /// "professional"; keeping these in sync avoids an empty Settings picker.
    static let all: [AirNoteTone] = [
        AirNoteTone(key: "neutral", label: "Neutral", detail: "Clear and balanced"),
        AirNoteTone(key: "professional", label: "Professional", detail: "Formal and polished — great for work"),
        AirNoteTone(key: "casual", label: "Casual", detail: "Friendly and conversational"),
        AirNoteTone(key: "assertive", label: "Assertive", detail: "Direct and confident"),
        AirNoteTone(key: "concise", label: "Concise", detail: "Minimal words, every one earns its place"),
        AirNoteTone(key: "custom", label: "Custom", detail: "Write your own persona instructions"),
    ]

    static func label(for key: String) -> String {
        all.first { $0.key == key }?.label ?? key.capitalized
    }

    /// Coerce a stale/unknown tone value (e.g. the legacy "work"/"email"/"notes"
    /// vocabulary, or a default that predates this list) to a real picker tag, so
    /// the Settings/Profile tone picker can never render blank.
    static func coerced(_ key: String) -> String {
        if all.contains(where: { $0.key == key }) { return key }
        switch key {
        case "work", "email": return "professional"
        case "notes": return "concise"
        default: return "professional"
        }
    }
}

// MARK: - Stat tile

struct StatTile: View {
    var value: String
    var label: String
    var systemImage: String
    var tint: Color = AirNoteDesign.accent

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Image(systemName: systemImage)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(tint)
            Text(value)
                .font(.system(size: 22, weight: .bold, design: .rounded))
                .foregroundStyle(AirNoteDesign.foreground)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(label)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(AirNoteDesign.muted)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: AirNoteDesign.tileRadius, style: .continuous)
                .fill(AirNoteDesign.surface.opacity(0.92))
        )
        .overlay(
            RoundedRectangle(cornerRadius: AirNoteDesign.tileRadius, style: .continuous)
                .strokeBorder(AirNoteDesign.border, lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(value) \(label)")
    }
}

// MARK: - Section header

struct SectionHeader: View {
    var title: String
    var action: (() -> Void)?
    var actionLabel: String?

    init(_ title: String, actionLabel: String? = nil, action: (() -> Void)? = nil) {
        self.title = title
        self.actionLabel = actionLabel
        self.action = action
    }

    var body: some View {
        HStack {
            AirNoteSectionLabel(text: title)
            Spacer()
            if let action, let actionLabel {
                Button(actionLabel, action: action)
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(AirNoteDesign.accent)
            }
        }
    }
}

// MARK: - Chip

struct AirNoteChip: View {
    var text: String
    var tint: Color = AirNoteDesign.accent

    var body: some View {
        Text(text)
            .font(.caption2.weight(.bold))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(tint.opacity(0.12), in: Capsule())
    }
}

// MARK: - Empty state

struct EmptyStateCard: View {
    var systemImage: String
    var title: String
    var message: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 30, weight: .regular))
                .foregroundStyle(AirNoteDesign.muted)
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(AirNoteDesign.foreground)
            Text(message)
                .font(.caption)
                .foregroundStyle(AirNoteDesign.muted)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 30)
        .padding(.horizontal, 18)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - 14-day activity chart

struct ActivityChart: View {
    /// One value per day, oldest → newest.
    var values: [Int]
    var labels: [String] = []

    private var maxValue: Int { max(values.max() ?? 0, 1) }

    var body: some View {
        HStack(alignment: .bottom, spacing: 5) {
            ForEach(Array(values.enumerated()), id: \.offset) { index, value in
                VStack(spacing: 5) {
                    RoundedRectangle(cornerRadius: 3, style: .continuous)
                        .fill(value > 0 ? AirNoteDesign.accent.opacity(0.9) : AirNoteDesign.surfaceRaised)
                        .frame(height: barHeight(for: value))
                        .frame(maxWidth: .infinity)
                    if index < labels.count {
                        Text(labels[index])
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(AirNoteDesign.muted)
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(dayDescription(index)): \(value) dictation\(value == 1 ? "" : "s")")
            }
        }
        .frame(height: 78)
    }

    private func barHeight(for value: Int) -> CGFloat {
        let proportion = CGFloat(value) / CGFloat(maxValue)
        return max(6, proportion * 60)
    }

    private func dayDescription(_ index: Int) -> String {
        let daysAgo = values.count - 1 - index
        if daysAgo == 0 { return "Today" }
        if daysAgo == 1 { return "Yesterday" }
        return "\(daysAgo) days ago"
    }
}

// MARK: - Loading row

struct InlineLoading: View {
    var text: String

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text)
                .font(.caption)
                .foregroundStyle(AirNoteDesign.muted)
        }
        .frame(maxWidth: .infinity, alignment: .center)
        .padding(.vertical, 16)
    }
}

// MARK: - Avatar

/// Profile avatar accent palette — a contained cosmetic personalization that
/// tints the avatar (not the whole app theme). Index 0 is the app default.
enum ProfileAccent {
    static let palette: [Color] = [
        AirNoteDesign.accent,
        Color(red: 0.55, green: 0.40, blue: 0.95), // purple
        Color(red: 0.93, green: 0.40, blue: 0.62), // pink
        Color(red: 0.16, green: 0.70, blue: 0.62), // teal
        Color(red: 0.96, green: 0.58, blue: 0.20), // amber
        Color(red: 0.27, green: 0.72, blue: 0.42), // green
    ]

    static func color(_ index: Int) -> Color {
        palette.indices.contains(index) ? palette[index] : palette[0]
    }
}

struct AccountAvatar: View {
    var email: String
    var size: CGFloat = 40
    /// Optional display name — its initial wins over the email's when present.
    var name: String? = nil
    /// Avatar tint; defaults to the app accent so existing call sites are unchanged.
    var tint: Color = AirNoteDesign.accent

    private var initial: String {
        let source = (name?.isEmpty == false) ? name! : email
        return String(source.first ?? "A").uppercased()
    }

    var body: some View {
        Circle()
            .fill(tint.opacity(0.16))
            .frame(width: size, height: size)
            .overlay(
                Text(initial)
                    .font(.system(size: size * 0.42, weight: .bold, design: .rounded))
                    .foregroundStyle(tint)
            )
            .accessibilityHidden(true)
    }
}
