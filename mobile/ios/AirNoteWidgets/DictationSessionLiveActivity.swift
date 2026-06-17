import ActivityKit
import AirNoteShared
import AppIntents
import SwiftUI
import WidgetKit

/// The Dynamic Island + Lock Screen presentation of the warm dictation session,
/// with Stop / Resume controls. The Dynamic Island always renders on a dark
/// background, so foreground colors here are light regardless of the app palette.
@available(iOS 16.1, *)
struct DictationSessionLiveActivity: Widget {
    private static let accent = Color(red: 0.62, green: 0.70, blue: 0.98)

    var body: some WidgetConfiguration {
        ActivityConfiguration(for: DictationSessionAttributes.self) { context in
            LockScreenView(state: context.state)
                .activityBackgroundTint(Color.black.opacity(0.86))
                .activitySystemActionForegroundColor(.white)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    WaveMark(color: Self.accent)
                        .frame(width: 30, height: 26)
                        .padding(.leading, 4)
                        .opacity(context.state.active ? 1 : 0.45)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Image(systemName: context.state.active ? "waveform" : "pause.circle.fill")
                        .font(.title3)
                        .foregroundStyle(Self.accent)
                        .padding(.trailing, 6)
                }
                DynamicIslandExpandedRegion(.center) {
                    VStack(spacing: 2) {
                        Text("AirNote")
                            .font(.subheadline.weight(.bold))
                            .foregroundStyle(.white)
                        Text(context.state.active ? "Session on" : "Paused")
                            .font(.caption2)
                            .foregroundStyle(.white.opacity(0.7))
                    }
                }
                DynamicIslandExpandedRegion(.bottom) {
                    SessionButton(active: context.state.active, accent: Self.accent)
                }
            } compactLeading: {
                WaveMark(color: Self.accent)
                    .frame(width: 20, height: 16)
                    .opacity(context.state.active ? 1 : 0.45)
            } compactTrailing: {
                Image(systemName: context.state.active ? "waveform" : "pause.fill")
                    .foregroundStyle(Self.accent)
            } minimal: {
                WaveMark(color: Self.accent)
                    .frame(width: 16, height: 14)
                    .opacity(context.state.active ? 1 : 0.45)
            }
            .keylineTint(Self.accent)
        }
    }
}

/// The Stop / Resume control, driven by App Intents.
@available(iOS 16.1, *)
private struct SessionButton: View {
    var active: Bool
    var accent: Color

    var body: some View {
        if #available(iOS 17.0, *) {
            if active {
                Button(intent: StopSessionIntent()) {
                    Label("Stop session", systemImage: "stop.fill")
                        .font(.subheadline.weight(.semibold))
                }
                .tint(accent)
            } else {
                Button(intent: ResumeSessionIntent()) {
                    Label("Resume session", systemImage: "mic.fill")
                        .font(.subheadline.weight(.semibold))
                }
                .tint(accent)
            }
        } else {
            Text(active ? "Tap the AirNote mic in any app" : "Open AirNote to resume")
                .font(.caption2)
                .foregroundStyle(.white.opacity(0.6))
        }
    }
}

@available(iOS 16.1, *)
private struct LockScreenView: View {
    var state: DictationSessionAttributes.ContentState
    private let accent = Color(red: 0.62, green: 0.70, blue: 0.98)

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(LinearGradient(
                        colors: [accent, Color(red: 0.42, green: 0.52, blue: 0.92)],
                        startPoint: .top, endPoint: .bottom
                    ))
                    .frame(width: 40, height: 40)
                    .opacity(state.active ? 1 : 0.5)
                WaveMark(color: .white).frame(width: 22, height: 18)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text("AirNote")
                    .font(.headline)
                    .foregroundStyle(.white)
                Text(state.active ? "Session on — tap the AirNote mic in any app" : "Session paused")
                    .font(.caption)
                    .foregroundStyle(.white.opacity(0.7))
            }
            Spacer(minLength: 8)
            SessionButton(active: state.active, accent: accent)
        }
        .padding(14)
    }
}

/// A small 4-bar waveform "logo" mark.
private struct WaveMark: View {
    var color: Color
    private let heights: [CGFloat] = [0.45, 1.0, 0.7, 0.55]

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: geo.size.width * 0.12) {
                ForEach(heights.indices, id: \.self) { i in
                    Capsule()
                        .fill(color)
                        .frame(height: geo.size.height * heights[i])
                        .frame(maxHeight: .infinity, alignment: .center)
                }
            }
        }
    }
}
