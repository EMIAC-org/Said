import SwiftUI

/// Explains — and shows live status for — the speech model that runs on the
/// iPhone, and exactly which parts of a dictation run on the phone vs the cloud.
struct OnDeviceAIScreen: View {
    @ObservedObject private var model = OnDeviceModelStatus.shared

    var body: some View {
        ZStack {
            AirNoteBackground()
            ScrollView {
                VStack(spacing: 18) {
                    hero
                    statusCard
                    ScreenSectionTitle(title: "What runs where")
                    whereCard(
                        place: .onDevice,
                        title: "On your iPhone",
                        subtitle: "Private, and works without internet",
                        rows: [
                            ("mic.fill", "Recording", "Your voice is captured and kept only in memory."),
                            ("waveform.badge.magnifyingglass", "Silence detection", "\(OnDeviceModelStatus.vadName) trims pauses so nothing is made up."),
                            ("waveform", "Speech to text", "\(OnDeviceModelStatus.modelName) turns Hindi, English and Hinglish into text."),
                            ("textformat", "Roman Hinglish", "Any Devanagari is written in Roman letters."),
                            ("person.text.rectangle", "Your corrections", "Names and fixes you've taught AirNote are applied."),
                        ]
                    )
                    whereCard(
                        place: .cloud,
                        title: "In AirNote Cloud",
                        subtitle: "Only text is sent — never audio",
                        rows: [
                            ("sparkles", "Polish", "Punctuation, grammar and your chosen tone."),
                            ("character.book.closed", "Your Dictionary", "Spells your names and terms the way you write them."),
                            ("clock.arrow.circlepath", "History sync", "Your dictations on every device."),
                            ("brain", "Learning", "Learns from the edits you make."),
                        ]
                    )
                    ScreenSectionTitle(title: "Model details")
                    detailsCard
                    privacyCard
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 32)
            }
        }
        .navigationTitle("On-device AI")
        .navigationBarTitleDisplayMode(.inline)
    }

    // MARK: Hero

    private var hero: some View {
        VStack(spacing: 14) {
            ZStack {
                Circle()
                    .fill(AirNoteDesign.onDevice.opacity(0.12))
                    .frame(width: 112, height: 112)
                Circle()
                    .strokeBorder(AirNoteDesign.onDevice.opacity(0.25), lineWidth: 1)
                    .frame(width: 112, height: 112)
                Image(systemName: "iphone.gen3")
                    .font(.system(size: 50, weight: .light))
                    .foregroundStyle(AirNoteDesign.onDevice)
                Image(systemName: "waveform")
                    .font(.system(size: 18, weight: .bold))
                    .foregroundStyle(AirNoteDesign.onDevice)
                    .symbolEffect(.variableColor.iterative, options: .repeating, isActive: model.state == .loading)
            }
            .padding(.top, 8)
            Text("Your voice is understood\nright on your iPhone")
                .font(.system(.title2, design: .rounded).weight(.bold))
                .multilineTextAlignment(.center)
                .foregroundStyle(AirNoteDesign.foreground)
            Text("AirNote runs its own Hinglish speech model on the phone. Your audio never leaves the device, and dictation keeps working offline.")
                .font(.subheadline)
                .foregroundStyle(AirNoteDesign.muted)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 8)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 6)
    }

    // MARK: Live status

    private var statusCard: some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 10) {
                    StatusDot(color: model.stateColor, pulsing: model.state == .ready)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(OnDeviceModelStatus.modelName)
                            .font(.system(.headline, design: .rounded))
                            .foregroundStyle(AirNoteDesign.foreground)
                        Text(model.stateLabel)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(model.stateColor)
                    }
                    Spacer()
                    PlaceBadge(place: .onDevice)
                }
                HStack(spacing: 10) {
                    metric(
                        value: model.realtimeFactor.map { String(format: "%.1f×", $0) } ?? "—",
                        label: "faster than\nreal time"
                    )
                    metric(
                        value: model.lastDecodeMS.map { String(format: "%.1fs", Double($0) / 1000) } ?? "—",
                        label: "last\ntranscription"
                    )
                    metric(
                        value: model.totalRuns > 0 ? model.totalAudioLabel : "—",
                        label: "speech\ntranscribed"
                    )
                }
                if let last = model.lastRunLabel {
                    Label(last, systemImage: "bolt.fill")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                } else {
                    Label("Stats appear after your first dictation.", systemImage: "info.circle")
                        .font(.caption)
                        .foregroundStyle(AirNoteDesign.muted)
                }
            }
        }
    }

    private func metric(value: String, label: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(value)
                .font(.system(size: 22, weight: .bold, design: .rounded))
                .foregroundStyle(AirNoteDesign.foreground)
                .contentTransition(.numericText())
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(AirNoteDesign.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(AirNoteDesign.surfaceRaised.opacity(0.6), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    // MARK: Where things run

    private func whereCard(place: ProcessingPlace, title: String, subtitle: String, rows: [(String, String, String)]) -> some View {
        AirNoteCard(padding: 18) {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    IconTile(systemImage: place == .onDevice ? "iphone" : "cloud.fill", tint: place.color, size: 40)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .font(.system(.headline, design: .rounded))
                            .foregroundStyle(AirNoteDesign.foreground)
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(place.color)
                    }
                }
                Divider().overlay(AirNoteDesign.border)
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(rows, id: \.1) { icon, name, detail in
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: icon)
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(place.color)
                                .frame(width: 22)
                                .padding(.top, 1)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(name)
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(AirNoteDesign.foreground)
                                Text(detail)
                                    .font(.caption)
                                    .foregroundStyle(AirNoteDesign.muted)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
    }

    // MARK: Details

    private var detailsCard: some View {
        AirNoteCard(padding: 18) {
            VStack(spacing: 12) {
                FactRow(label: "Model", value: OnDeviceModelStatus.modelName)
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Based on", value: OnDeviceModelStatus.modelBase)
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Trained on", value: OnDeviceModelStatus.trainingHours)
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Languages", value: OnDeviceModelStatus.languages)
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Size", value: "\(model.sizeLabel) · \(OnDeviceModelStatus.quantization)")
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Engine", value: OnDeviceModelStatus.engine)
                Divider().overlay(AirNoteDesign.border)
                FactRow(label: "Version", value: OnDeviceModelStatus.revision)
                if let loadMS = model.loadMS {
                    Divider().overlay(AirNoteDesign.border)
                    FactRow(label: "Load time", value: "\(loadMS) ms")
                }
            }
        }
    }

    private var privacyCard: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "lock.shield.fill")
                .font(.system(size: 22))
                .foregroundStyle(AirNoteDesign.onDevice)
            VStack(alignment: .leading, spacing: 4) {
                Text("Audio never leaves this iPhone")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(AirNoteDesign.foreground)
                Text("Recordings are transcribed in memory and discarded. Without internet you still get your exact words — only polish needs a connection.")
                    .font(.caption)
                    .foregroundStyle(AirNoteDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AirNoteDesign.onDevice.opacity(0.10), in: RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous).strokeBorder(AirNoteDesign.onDevice.opacity(0.22), lineWidth: 1))
    }
}

/// Compact Home card that summarizes the on-device model and links to details.
struct OnDeviceModelCard: View {
    @ObservedObject private var model = OnDeviceModelStatus.shared

    var body: some View {
        NavigationLink(destination: OnDeviceAIScreen()) {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    IconTile(systemImage: "cpu", tint: AirNoteDesign.onDevice, size: 42)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("On-device AI")
                            .font(.system(.headline, design: .rounded))
                            .foregroundStyle(AirNoteDesign.foreground)
                        HStack(spacing: 6) {
                            StatusDot(color: model.stateColor, pulsing: model.state == .ready)
                            Text("\(OnDeviceModelStatus.modelName) · \(model.stateLabel)")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(AirNoteDesign.muted)
                        }
                    }
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(AirNoteDesign.muted)
                }
                HStack(spacing: 8) {
                    chip("lock.fill", "Private")
                    chip("wifi.slash", "Works offline")
                    chip("bolt.fill", model.realtimeFactor.map { String(format: "%.1f× real time", $0) } ?? model.sizeLabel)
                }
            }
            .padding(18)
            .background(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .fill(AirNoteDesign.surface)
                    .shadow(color: AirNoteDesign.cardShadow.opacity(0.55), radius: 18, x: 0, y: 8)
            )
            .overlay(
                RoundedRectangle(cornerRadius: AirNoteDesign.cardRadius, style: .continuous)
                    .strokeBorder(AirNoteDesign.onDevice.opacity(0.25), lineWidth: 1)
            )
        }
        .buttonStyle(PressScaleStyle())
        .accessibilityLabel("On-device AI, \(OnDeviceModelStatus.modelName), \(model.stateLabel). Opens details.")
    }

    private func chip(_ icon: String, _ text: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: icon).font(.system(size: 9, weight: .bold))
            Text(text).font(.system(size: 11, weight: .semibold, design: .rounded)).lineLimit(1)
        }
        .foregroundStyle(AirNoteDesign.onDevice)
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(AirNoteDesign.onDevice.opacity(0.11), in: Capsule())
    }
}
