import Combine
import Sparkle
import SwiftUI

@MainActor
final class SoftwareUpdateManager: ObservableObject {
    static let shared = SoftwareUpdateManager()

    let updaterController: SPUStandardUpdaterController?

    var updater: SPUUpdater? {
        updaterController?.updater
    }

    var isConfigured: Bool {
        updaterController != nil
    }

    private init() {
        guard Self.bundleHasSparkleConfiguration else {
            updaterController = nil
            return
        }

        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
    }

    private static var bundleHasSparkleConfiguration: Bool {
        guard
            let feedURL = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String,
            let publicKey = Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String
        else {
            return false
        }

        return !feedURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !publicKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

@MainActor
private final class CheckForUpdatesViewModel: ObservableObject {
    @Published var canCheckForUpdates = false

    private var cancellable: AnyCancellable?

    init(updater: SPUUpdater?) {
        guard let updater else { return }
        canCheckForUpdates = updater.canCheckForUpdates
        cancellable = updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .assign(to: \.canCheckForUpdates, on: self)
    }
}

@MainActor
struct SoftwareUpdateButton: View {
    let manager: SoftwareUpdateManager
    @StateObject private var viewModel: CheckForUpdatesViewModel

    init(manager: SoftwareUpdateManager) {
        self.manager = manager
        _viewModel = StateObject(wrappedValue: CheckForUpdatesViewModel(updater: manager.updater))
    }

    var body: some View {
        if let updater = manager.updater {
            Button("Check for Updates...") {
                updater.checkForUpdates()
            }
            .disabled(!viewModel.canCheckForUpdates)
        } else {
            Button("Check for Updates...") {}
                .disabled(true)
        }
    }
}

@MainActor
struct SoftwareUpdateSettingsSection: View {
    let manager: SoftwareUpdateManager

    @State private var automaticallyChecksForUpdates = false
    @State private var automaticallyDownloadsUpdates = false

    var body: some View {
        Section {
            if let updater = manager.updater {
                Toggle("Automatically check for updates", isOn: $automaticallyChecksForUpdates)
                    .onChange(of: automaticallyChecksForUpdates) { _, newValue in
                        updater.automaticallyChecksForUpdates = newValue
                        if !newValue {
                            automaticallyDownloadsUpdates = false
                            updater.automaticallyDownloadsUpdates = false
                        }
                    }

                Toggle("Automatically download updates", isOn: $automaticallyDownloadsUpdates)
                    .disabled(!automaticallyChecksForUpdates)
                    .onChange(of: automaticallyDownloadsUpdates) { _, newValue in
                        updater.automaticallyDownloadsUpdates = newValue
                    }
            } else {
                Text("Updates are not configured in this build.")
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Software Updates")
        }
        .onAppear(perform: refresh)
    }

    private func refresh() {
        guard let updater = manager.updater else {
            automaticallyChecksForUpdates = false
            automaticallyDownloadsUpdates = false
            return
        }

        automaticallyChecksForUpdates = updater.automaticallyChecksForUpdates
        automaticallyDownloadsUpdates = updater.automaticallyDownloadsUpdates
    }
}
