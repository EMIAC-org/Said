import SwiftUI

struct RecordingSessionView: View {
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 14) {
                    HStack {
                        AirNoteStatusPill(systemImage: "waveform", text: "Session live", color: AirNoteDesign.accent)
                        Spacer()
                        Text("5 min")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }

                    Text("Swipe back to your app")
                        .font(.title2.weight(.semibold))

                    Text("AirNote Keyboard will use this visible session to record after you tap the mic. If iOS pauses this app, the keyboard will ask you to restart instead of hanging.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    LevelPreview()

                    AirNoteActionRow(
                        primaryTitle: "I am back in my app",
                        primarySystemImage: "arrowshape.turn.up.backward.fill",
                        secondaryTitle: "Cancel session",
                        secondarySystemImage: "xmark.circle",
                        primaryAction: {},
                        secondaryAction: {}
                    )
                }
                .padding(14)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))

                VStack(alignment: .leading, spacing: 10) {
                    Text("Session health")
                        .font(.headline)
                    HealthRow(systemImage: "mic.fill", title: "Microphone", value: "Ready")
                    HealthRow(systemImage: "keyboard", title: "Keyboard bridge", value: "Heartbeat active")
                    HealthRow(systemImage: "lock.shield", title: "Privacy", value: "Records only after tap")
                }
                .padding(14)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
            }
            .padding(16)
        }
        .navigationTitle("AirNote Session")
        .background(Color(.systemGroupedBackground))
    }
}

private struct LevelPreview: View {
    private let levels: [CGFloat] = [0.25, 0.58, 0.86, 0.48, 0.72, 0.34, 0.62]

    var body: some View {
        HStack(alignment: .center, spacing: 7) {
            ForEach(levels.indices, id: \.self) { index in
                Capsule()
                    .fill(index == 2 ? AirNoteDesign.accent : AirNoteDesign.teal.opacity(0.55))
                    .frame(width: 8, height: 52 * levels[index])
                    .accessibilityHidden(true)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 58)
        .padding(.vertical, 8)
        .background(Color(.tertiarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: AirNoteDesign.radius, style: .continuous))
        .accessibilityLabel("Microphone level preview")
    }
}

private struct HealthRow: View {
    var systemImage: String
    var title: String
    var value: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .foregroundStyle(AirNoteDesign.accent)
                .frame(width: 22)
            Text(title)
                .font(.subheadline)
            Spacer()
            Text(value)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }
}
