import SwiftUI
import UIKit

struct MainTabView: View {
    @EnvironmentObject private var env: AppEnvironment

    init() { Self.styleTabBar() }

    /// A clean, blurred, shadowless tab bar with accent selection (Wispr-style).
    private static func styleTabBar() {
        let appearance = UITabBarAppearance()
        appearance.configureWithDefaultBackground()
        appearance.backgroundEffect = UIBlurEffect(style: .systemUltraThinMaterial)
        appearance.shadowColor = .clear
        let accent = UIColor(AirNoteDesign.accent)
        let muted = UIColor(AirNoteDesign.muted)
        for item in [appearance.stackedLayoutAppearance, appearance.inlineLayoutAppearance, appearance.compactInlineLayoutAppearance] {
            item.selected.iconColor = accent
            item.selected.titleTextAttributes = [.foregroundColor: accent]
            item.normal.iconColor = muted
            item.normal.titleTextAttributes = [.foregroundColor: muted]
        }
        UITabBar.appearance().standardAppearance = appearance
        UITabBar.appearance().scrollEdgeAppearance = appearance
    }

    var body: some View {
        TabView(selection: $env.selectedTab) {
            DashboardScreen()
                .tabItem { Label("Home", systemImage: "waveform") }
                .tag(0)
            HistoryScreen()
                .tabItem { Label("History", systemImage: "clock.arrow.circlepath") }
                .tag(1)
            VocabularyScreen()
                .tabItem { Label("Words", systemImage: "character.book.closed.fill") }
                .tag(2)
            InsightsScreen()
                .tabItem { Label("Insights", systemImage: "chart.bar.xaxis") }
                .tag(3)
            SettingsScreen()
                .tabItem { Label("Settings", systemImage: "gearshape.fill") }
                .tag(4)
        }
        .tint(AirNoteDesign.accent)
        .sensoryFeedback(.selection, trigger: env.selectedTab)
    }
}
