import SwiftUI

struct MainTabView: View {
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
