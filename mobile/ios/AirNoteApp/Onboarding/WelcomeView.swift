import SwiftUI

struct WelcomeView: View {
    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 22) {
                    VStack(spacing: 16) {
                        RoundedRectangle(cornerRadius: 24, style: .continuous)
                            .fill(AirNoteDesign.accentGradient)
                            .frame(width: 88, height: 88)
                            .overlay(
                                Image(systemName: "waveform")
                                    .font(.system(size: 40, weight: .bold))
                                    .foregroundStyle(.white)
                            )
                            .shadow(color: AirNoteDesign.accent.opacity(0.42), radius: 22, x: 0, y: 12)

                        Text("Speak naturally.\nAirNote writes clearly.")
                            .font(.system(.largeTitle, design: .rounded).weight(.bold))
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)

                        Text("A calm voice keyboard for English, Hindi, and Hinglish — polished on AirNote's hosted Gateway.")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 8)
                    }
                    .padding(.top, 24)

                    VStack(spacing: 12) {
                        FeatureRow(icon: "mic.fill", tint: AirNoteDesign.accent,
                                   title: "Speak, don't type", subtitle: "Hold the mic and talk — naturally.")
                        FeatureRow(icon: "sparkles", tint: AirNoteDesign.accent2,
                                   title: "Polished instantly", subtitle: "Clean, readable text in your chosen style.")
                        FeatureRow(icon: "lock.shield.fill", tint: AirNoteDesign.success,
                                   title: "Private by design", subtitle: "No recording in secure fields; nothing stored by default.")
                    }
                }
                .padding(18)
            }
        }
        .navigationTitle("Welcome")
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct FeatureRow: View {
    var icon: String
    var tint: Color
    var title: String
    var subtitle: String

    var body: some View {
        AirNoteCard(padding: 16) {
            HStack(spacing: 14) {
                ZStack {
                    Circle().fill(tint.opacity(0.15)).frame(width: 44, height: 44)
                    Image(systemName: icon).font(.headline).foregroundStyle(tint)
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.subheadline.weight(.semibold))
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
        }
    }
}
