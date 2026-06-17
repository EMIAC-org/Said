import SwiftUI
import UIKit

struct MainTabView: View {
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
        TabView {
            DashboardScreen()
                .tabItem { Label("Home", systemImage: "house.fill") }
            HistoryScreen()
                .tabItem { Label("History", systemImage: "clock.arrow.circlepath") }
            VocabularyScreen()
                .tabItem { Label("Words", systemImage: "textformat.abc") }
            InsightsScreen()
                .tabItem { Label("Insights", systemImage: "chart.bar.fill") }
            SettingsScreen()
                .tabItem { Label("Settings", systemImage: "gearshape.fill") }
        }
        .tint(AirNoteDesign.accent)
    }
}
