import SwiftUI

enum AirNoteDesign {
    static let accent = Color(red: 0.31, green: 0.40, blue: 0.92)
    static let teal = Color(red: 0.07, green: 0.62, blue: 0.85)
    static let success = Color(red: 0.05, green: 0.52, blue: 0.30)
    static let warning = Color(red: 0.82, green: 0.46, blue: 0.0)
    static let danger = Color(red: 0.78, green: 0.17, blue: 0.14)
    static let radius: CGFloat = 8
}

struct AirNoteStatusPill: View {
    var systemImage: String
    var text: String
    var color: Color = AirNoteDesign.accent

    var body: some View {
        Label(text, systemImage: systemImage)
            .font(.caption.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(color.opacity(0.12), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
            .accessibilityElement(children: .combine)
    }
}

struct AirNoteActionRow: View {
    var primaryTitle: String
    var primarySystemImage: String
    var secondaryTitle: String
    var secondarySystemImage: String
    var primaryAction: () -> Void
    var secondaryAction: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Button(action: primaryAction) {
                Label(primaryTitle, systemImage: primarySystemImage)
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)

            Button(action: secondaryAction) {
                Label(secondaryTitle, systemImage: secondarySystemImage)
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
        }
    }
}
