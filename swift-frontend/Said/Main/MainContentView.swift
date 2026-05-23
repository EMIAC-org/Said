import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case dashboard = "Dashboard"
    case history = "History"
    case insights = "Insights"
    case vocabulary = "Vocabulary"
    case settings = "Settings"

    var id: String { rawValue }

    var icon: String {
        switch self {
        case .dashboard: return "square.grid.2x2"
        case .history: return "clock"
        case .insights: return "chart.bar"
        case .vocabulary: return "textformat.abc"
        case .settings: return "gear"
        }
    }
}

struct MainContentView: View {
    let sidecar: SidecarManager
    let engine: DictationEngine
    let updateManager: SoftwareUpdateManager

    @State private var selected: SidebarItem = .dashboard

    var body: some View {
        NavigationSplitView {
            List(SidebarItem.allCases, selection: $selected) { item in
                Label(item.rawValue, systemImage: item.icon)
                    .tag(item)
            }
            .navigationSplitViewColumnWidth(180)
        } detail: {
            Group {
                switch selected {
                case .dashboard:
                    DashboardView(sidecar: sidecar)
                case .history:
                    HistoryView(sidecar: sidecar)
                case .insights:
                    InsightsView(sidecar: sidecar)
                case .vocabulary:
                    VocabularyView(sidecar: sidecar)
                case .settings:
                    SettingsView(sidecar: sidecar, engine: engine, updateManager: updateManager)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}
