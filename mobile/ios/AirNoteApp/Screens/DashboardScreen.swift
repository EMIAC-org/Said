import AirNoteShared
import SwiftUI

struct DashboardScreen: View {
    @EnvironmentObject private var env: AppEnvironment
    @ObservedObject private var warmHost = WarmDictationHost.shared
    @State private var showDictation = false
    @State private var showOnDevice = false

    private var stats: DictationStats { DictationStats(items: env.history) }

    var body: some View {
        NavigationStack {
            ZStack {
                AirNoteBackground()
                ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 18) {
                        header
                        heroCard
                        OnDeviceModelCard()
                        keyboardCard
                        if !env.history.isEmpty {
                            ScreenSectionTitle(title: "Your week")
                            statsGrid
                            activityCard
                        }
                        ScreenSectionTitle(
                            title: "Recent",
                            trailing: env.history.isEmpty ? nil : "See all",
                            action: { env.selectedTab = 1 }
                        )
                        recentCard.id("recent")
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .padding(.bottom, 32)
                }
                .onAppear {
                    #if DEBUG
                    if AppEnvironment.demoRoute == "recent" {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { proxy.scrollTo("recent", anchor: .bottom) }
                    }
                    #endif
                }
                }
            }
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(isPresented: $showOnDevice) { OnDeviceAIScreen() }
            .task {
                #if DEBUG
                switch AppEnvironment.demoRoute {
                case "dictation": showDictation = true
                case "ondevice": showOnDevice = true
                default: break
                }
                #endif
                await env.refreshHistory()
            }
            .refreshable { await env.refreshHistory() }
        }
        .sheet(isPresented: $showDictation) {
            NavigationStack { DictationSheet(env: env, showsDoneButton: false) }
                .presentationDragIndicator(.visible)
                .presentationCornerRadius(32)
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(greeting)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(AirNoteDesign.muted)
                Text(firstName)
                    .font(.system(size: 30, weight: .bold, design: .rounded))
                    .foregroundStyle(AirNoteDesign.foreground)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            Spacer()
            NavigationLink {
                ProfileScreen()
            } label: {
                AccountAvatar(
                    email: env.account?.email ?? "A",
                    size: 44,
                    name: env.profileDisplayName,
                    tint: ProfileAccent.color(env.profileAccentIndex)
                )
                .overlay(Circle().strokeBorder(AirNoteDesign.border, lineWidth: 1))
            }
            .accessibilityLabel("Your profile")
        }
        .padding(.horizontal, 4)
        .padding(.top, 6)
    }

    private var firstName: String {
        let name = env.profileDisplayName.trimmingCharacters(in: .whitespaces)
        if !name.isEmpty { return String(name.split(separator: " ").first ?? Substring(name)) }
        let local = (env.account?.email ?? "").split(separator: "@").first.map(String.init) ?? ""
        return local.isEmpty ? "Welcome" : local.prefix(1).uppercased() + local.dropFirst()
    }

    // MARK: Hero

    private var heroCard: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Speak. AirNote writes.")
                        .font(.system(size: 26, weight: .bold, design: .rounded))
                        .foregroundStyle(.white)
                    Text("Hindi, English or Hinglish — understood on your iPhone, polished in a second.")
                        .font(.subheadline)
                        .foregroundStyle(Color.white.opacity(0.78))
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            PipelineStrip(onDark: true)
                .padding(.vertical, 12)
                .padding(.horizontal, 6)
                .background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            Button {
                Haptics.tap()
                showDictation = true
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: "mic.fill")
                        .font(.system(size: 17, weight: .bold))
                    Text("Start dictation")
                }
                .font(.system(.body, design: .rounded).weight(.bold))
                .foregroundStyle(Color(red: 0.22, green: 0.20, blue: 0.55))
                .frame(maxWidth: .infinity)
                .frame(height: 54)
                .background(Color.white, in: RoundedRectangle(cornerRadius: AirNoteDesign.buttonRadius, style: .continuous))
                .shadow(color: Color.black.opacity(0.25), radius: 12, x: 0, y: 6)
            }
            .buttonStyle(PressScaleStyle())
        }
        .padding(20)
        .background(
            ZStack {
                AirNoteDesign.heroGradient
                RadialGradient(colors: [Color.white.opacity(0.22), .clear], center: .topTrailing, startRadius: 10, endRadius: 260)
            }
            .clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
        )
        .overlay(RoundedRectangle(cornerRadius: 28, style: .continuous).strokeBorder(Color.white.opacity(0.14), lineWidth: 1))
        .shadow(color: Color(red: 0.30, green: 0.22, blue: 0.70).opacity(0.35), radius: 24, x: 0, y: 14)
    }

    // MARK: Keyboard

    @ViewBuilder private var keyboardCard: some View {
        if env.permissions.keyboard != .ready {
            NavigationLink(destination: KeyboardSetupGuide()) {
                AirNoteCard(padding: 18) {
                    HStack(spacing: 14) {
                        IconTile(systemImage: "keyboard.badge.ellipsis", tint: AirNoteDesign.accent, size: 42)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Add the AirNote Keyboard")
                                .font(.system(.headline, design: .rounded))
                                .foregroundStyle(AirNoteDesign.foreground)
                            Text("Dictate into WhatsApp, Mail, Slack — any app.")
                                .font(.caption)
                                .foregroundStyle(AirNoteDesign.muted)
                        }
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption.weight(.bold)).foregroundStyle(AirNoteDesign.muted)
                    }
                }
            }
            .buttonStyle(PressScaleStyle())
        } else {
            AirNoteCard(padding: 18) {
                HStack(spacing: 14) {
                    IconTile(systemImage: "keyboard.fill", tint: warmHost.isSessionActive ? AirNoteDesign.success : AirNoteDesign.muted, size: 42)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Keyboard session")
                            .font(.system(.headline, design: .rounded))
                            .foregroundStyle(AirNoteDesign.foreground)
                        Text(warmHost.isSessionActive
                             ? "Live — tap the mic on the AirNote keyboard in any app."
                             : "Off — turn on to dictate from the keyboard without opening AirNote.")
                            .font(.caption)
                            .foregroundStyle(AirNoteDesign.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 4)
                    Toggle("", isOn: sessionBinding)
                        .labelsHidden()
                        .tint(AirNoteDesign.success)
                }
            }
            .accessibilityElement(children: .combine)
        }
    }

    private var sessionBinding: Binding<Bool> {
        Binding(
            get: { warmHost.isSessionActive },
            set: { on in
                Haptics.tap()
                Task {
                    if on, env.permissions.micPermission != .granted {
                        _ = await env.permissions.requestMic()
                    }
                    WarmDictationHost.shared.setSessionEnabled(on)
                }
            }
        )
    }

    // MARK: Stats

    private var statsGrid: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
            StatTile(value: stats.totalWords.compactString, label: "words dictated", systemImage: "text.alignleft")
            StatTile(value: stats.timeSavedLabel, label: "saved vs typing", systemImage: "hourglass", tint: AirNoteDesign.violet)
            StatTile(value: "\(stats.count)", label: stats.count == 1 ? "dictation" : "dictations", systemImage: "waveform", tint: AirNoteDesign.onDevice)
            StatTile(value: "\(stats.streak)", label: stats.streak == 1 ? "day streak" : "day streak", systemImage: "flame.fill", tint: AirNoteDesign.warning)
        }
    }

    private var activityCard: some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    Text("Last 14 days")
                        .font(.system(.subheadline, design: .rounded).weight(.semibold))
                        .foregroundStyle(AirNoteDesign.foreground)
                    Spacer()
                    Text("\(stats.activity.reduce(0, +)) dictations")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(AirNoteDesign.muted)
                }
                ActivityChart(values: stats.activity, labels: stats.activityLabels)
            }
        }
    }

    // MARK: Recent

    private var recentCard: some View {
        AirNoteCard(padding: 6) {
            if env.historyLoading && env.history.isEmpty {
                InlineLoading(text: "Loading your dictations…")
            } else if env.history.isEmpty {
                EmptyStateCard(
                    systemImage: "waveform",
                    title: "No dictations yet",
                    message: "Tap Start dictation and say anything — it shows up here."
                )
            } else {
                VStack(spacing: 0) {
                    ForEach(env.history.prefix(3)) { item in
                        DictationRow(item: item)
                            .padding(12)
                        if item.id != env.history.prefix(3).last?.id {
                            Divider().overlay(AirNoteDesign.border).padding(.leading, 12)
                        }
                    }
                }
            }
        }
    }

    private var greeting: String {
        let hour = Calendar.current.component(.hour, from: Date())
        switch hour {
        case 5..<12: return "Good morning"
        case 12..<17: return "Good afternoon"
        case 17..<22: return "Good evening"
        default: return "Hello"
        }
    }
}

struct DictationRow: View {
    var item: RuntimeHistoryItem

    private var sourceLabel: (String, String) {
        switch item.source {
        case "ios_keyboard": return ("keyboard", "Keyboard")
        case "ios_app": return ("iphone", "App")
        case let s where s.contains("desktop") || s.contains("mac") || s.contains("windows"): return ("laptopcomputer", "Desktop")
        default: return ("waveform", "Dictation")
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(item.displayText)
                .font(.body)
                .foregroundStyle(AirNoteDesign.foreground)
                .lineLimit(3)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 6) {
                Label(sourceLabel.1, systemImage: sourceLabel.0)
                    .labelStyle(.titleAndIcon)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.muted)
                Text("·").foregroundStyle(AirNoteDesign.muted)
                Text(item.createdAt, format: .relative(presentation: .named))
                    .font(.caption2)
                    .foregroundStyle(AirNoteDesign.muted)
                Spacer()
                Text("\(item.displayText.wordCount) words")
                    .font(.caption2.weight(.medium).monospacedDigit())
                    .foregroundStyle(AirNoteDesign.muted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Stats

struct DictationStats {
    let count: Int
    let totalWords: Int
    let streak: Int
    let activity: [Int]
    let activityLabels: [String]

    /// Minutes saved vs typing: ~40 wpm typing on a phone vs ~150 wpm speaking.
    var timeSavedLabel: String {
        let minutes = Double(totalWords) / 40 - Double(totalWords) / 150
        if minutes < 1 { return "<1m" }
        if minutes < 60 { return "\(Int(minutes.rounded()))m" }
        return String(format: "%.1fh", minutes / 60)
    }

    init(items: [RuntimeHistoryItem], days: Int = 14, calendar: Calendar = .current, now: Date = Date()) {
        count = items.count
        totalWords = items.reduce(0) { $0 + $1.displayText.wordCount }

        let today = calendar.startOfDay(for: now)
        let activeDays: Set<Int> = Set(items.compactMap { item -> Int? in
            // Ignore future-dated items (clock skew) so they can't corrupt the streak.
            guard let delta = calendar.dateComponents([.day], from: calendar.startOfDay(for: item.createdAt), to: today).day,
                  delta >= 0 else { return nil }
            return delta
        })

        // Streak: consecutive days back from today (or yesterday) with activity.
        var streakCount = 0
        var dayOffset = activeDays.contains(0) ? 0 : (activeDays.contains(1) ? 1 : -1)
        if dayOffset >= 0 {
            while activeDays.contains(dayOffset) {
                streakCount += 1
                dayOffset += 1
            }
        }
        streak = streakCount

        // Activity: one bucket per day, oldest → newest.
        var buckets = Array(repeating: 0, count: days)
        for item in items {
            if let delta = calendar.dateComponents([.day], from: calendar.startOfDay(for: item.createdAt), to: today).day,
               delta >= 0, delta < days {
                buckets[days - 1 - delta] += 1
            }
        }
        activity = buckets

        let formatter = DateFormatter()
        formatter.dateFormat = "EEEEE"
        activityLabels = (0..<days).map { index in
            let offset = days - 1 - index
            let date = calendar.date(byAdding: .day, value: -offset, to: today) ?? today
            return formatter.string(from: date)
        }
    }
}

extension String {
    var wordCount: Int {
        split { $0 == " " || $0 == "\n" || $0 == "\t" }.filter { !$0.isEmpty }.count
    }
}

extension Int {
    /// Compact display: 1234 → "1.2k".
    var compactString: String {
        if self >= 1_000_000 { return String(format: "%.1fM", Double(self) / 1_000_000) }
        if self >= 1_000 { return String(format: "%.1fk", Double(self) / 1_000) }
        return "\(self)"
    }
}
