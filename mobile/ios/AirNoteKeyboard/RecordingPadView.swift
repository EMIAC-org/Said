import UIKit
import AirNoteShared

final class RecordingPadView: UIView {
    var onStart: (() -> Void)?
    var onStop: (() -> Void)?
    var onInsert: (() -> Void)?
    var onCopy: (() -> Void)?
    var onSave: (() -> Void)?
    var onOpenApp: (() -> Void)?
    var onKeyTap: ((String) -> Void)?
    var onDelete: (() -> Void)?
    var onNextKeyboard: (() -> Void)?

    private let state: KeyboardState

    init(state: KeyboardState) {
        self.state = state
        super.init(frame: .zero)
        build()
    }

    required init?(coder: NSCoder) {
        self.state = .notConfigured
        super.init(coder: coder)
        build()
    }

    private func build() {
        backgroundColor = KeyboardTheme.keyboardBackground

        let stack = UIStackView()
        stack.axis = .vertical
        stack.spacing = 7
        stack.alignment = .fill
        stack.translatesAutoresizingMaskIntoConstraints = false

        stack.addArrangedSubview(makeVoiceSurface())
        makeKeyboardRows().forEach { stack.addArrangedSubview($0) }
        stack.addArrangedSubview(makeBottomRow())

        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8)
        ])
    }

    private func makeVoiceSurface() -> UIView {
        let surface = UIView()
        surface.backgroundColor = KeyboardTheme.surfaceBackground
        surface.layer.cornerRadius = KeyboardTheme.radius
        surface.layer.borderWidth = 1
        surface.layer.borderColor = KeyboardTheme.border.cgColor

        let stack = UIStackView()
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false

        stack.addArrangedSubview(makeStatusRow())
        stack.addArrangedSubview(makeWaveform())

        if let preview = previewText {
            stack.addArrangedSubview(makePreview(preview))
            stack.addArrangedSubview(makeResultActions())
        } else if let message = recoveryMessage {
            stack.addArrangedSubview(ErrorDrawer(message: message))
            stack.addArrangedSubview(makePrimaryActions())
        } else {
            stack.addArrangedSubview(makePrimaryActions())
        }

        surface.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: surface.leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: surface.trailingAnchor, constant: -10),
            stack.topAnchor.constraint(equalTo: surface.topAnchor, constant: 10),
            stack.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -10)
        ])
        return surface
    }

    private func makeStatusRow() -> UIView {
        let icon = UIImageView(image: UIImage(systemName: statusIcon))
        icon.tintColor = statusColor
        icon.contentMode = .scaleAspectFit
        icon.setContentHuggingPriority(.required, for: .horizontal)
        icon.widthAnchor.constraint(equalToConstant: 24).isActive = true

        let title = UILabel()
        title.font = .preferredFont(forTextStyle: .headline)
        title.adjustsFontForContentSizeCategory = true
        title.textColor = KeyboardTheme.foreground
        title.text = titleText
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let styleChip = makeChip(SharedStore.tonePreset.capitalized)
        let languageChip = makeChip(SharedStore.outputLanguage.capitalized)

        let row = UIStackView(arrangedSubviews: [icon, title, UIView(), styleChip, languageChip])
        row.axis = .horizontal
        row.spacing = 8
        row.alignment = .center
        row.accessibilityLabel = "\(titleText), \(subtitleText)"
        return row
    }

    private func makeWaveform() -> UIView {
        let container = UIView()
        container.backgroundColor = KeyboardTheme.secondarySurface
        container.layer.cornerRadius = KeyboardTheme.radius

        let label = UILabel()
        label.font = .preferredFont(forTextStyle: .caption1)
        label.adjustsFontForContentSizeCategory = true
        label.textColor = KeyboardTheme.muted
        label.text = subtitleText
        label.numberOfLines = 1

        let bars = UIStackView()
        bars.axis = .horizontal
        bars.alignment = .center
        bars.distribution = .equalCentering
        bars.spacing = 5

        for height in waveformHeights {
            let bar = UIView()
            bar.backgroundColor = statusColor.withAlphaComponent(isActiveWaveform ? 0.95 : 0.45)
            bar.layer.cornerRadius = 3
            bar.widthAnchor.constraint(equalToConstant: 6).isActive = true
            bar.heightAnchor.constraint(equalToConstant: height).isActive = true
            bars.addArrangedSubview(bar)
        }

        let row = UIStackView(arrangedSubviews: [bars, label])
        row.axis = .horizontal
        row.spacing = 10
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(row)

        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 10),
            row.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -10),
            row.topAnchor.constraint(equalTo: container.topAnchor, constant: 8),
            row.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -8),
            bars.widthAnchor.constraint(equalToConstant: 72)
        ])

        container.accessibilityLabel = subtitleText
        return container
    }

    private func makePreview(_ text: String) -> UIView {
        let label = UILabel()
        label.text = text
        label.font = .preferredFont(forTextStyle: .subheadline)
        label.adjustsFontForContentSizeCategory = true
        label.numberOfLines = 2
        label.textColor = KeyboardTheme.foreground
        label.backgroundColor = KeyboardTheme.secondarySurface
        label.layer.cornerRadius = KeyboardTheme.radius
        label.layer.masksToBounds = true
        label.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 8, leading: 10, bottom: 8, trailing: 10)
        label.accessibilityLabel = "Insert preview. \(text)"
        return label
    }

    private func makePrimaryActions() -> UIView {
        let primary = actionButton(title: primaryActionTitle, systemImage: primaryActionIcon, color: primaryActionColor)
        primary.addTarget(self, action: #selector(primaryTapped), for: .touchUpInside)
        primary.isEnabled = !isPrimaryActionDisabled
        primary.alpha = isPrimaryActionDisabled ? 0.55 : 1.0

        let row = UIStackView(arrangedSubviews: [primary])
        row.axis = .horizontal
        row.spacing = 8
        row.alignment = .fill
        return row
    }

    private func makeResultActions() -> UIView {
        if isCopyOnlyResult {
            let copy = actionButton(title: "Copy", systemImage: "doc.on.doc", color: KeyboardTheme.accent)
            copy.addTarget(self, action: #selector(copyTapped), for: .touchUpInside)

            let save = actionButton(title: "Save", systemImage: "tray.and.arrow.down", color: .secondaryLabel)
            save.addTarget(self, action: #selector(saveTapped), for: .touchUpInside)

            let row = UIStackView(arrangedSubviews: [copy, save])
            row.axis = .horizontal
            row.spacing = 8
            row.distribution = .fillEqually
            return row
        }

        let insert = actionButton(title: "Insert", systemImage: "text.insert", color: KeyboardTheme.accent)
        insert.addTarget(self, action: #selector(insertTapped), for: .touchUpInside)

        let copy = actionButton(title: "Copy", systemImage: "doc.on.doc", color: .secondaryLabel)
        copy.addTarget(self, action: #selector(copyTapped), for: .touchUpInside)

        let save = actionButton(title: "Save", systemImage: "tray.and.arrow.down", color: .secondaryLabel)
        save.addTarget(self, action: #selector(saveTapped), for: .touchUpInside)

        let row = UIStackView(arrangedSubviews: [insert, copy, save])
        row.axis = .horizontal
        row.spacing = 8
        row.distribution = .fillEqually
        return row
    }

    private func makeKeyboardRows() -> [UIView] {
        [
            makeKeyRow(["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"]),
            makeKeyRow(["A", "S", "D", "F", "G", "H", "J", "K", "L"]),
            makeKeyRow(["Z", "X", "C", "V", "B", "N", "M"])
        ]
    }

    private func makeKeyRow(_ keys: [String]) -> UIView {
        let row = UIStackView()
        row.axis = .horizontal
        row.spacing = 5
        row.distribution = .fillEqually
        for key in keys {
            row.addArrangedSubview(keyButton(key))
        }
        return row
    }

    private func makeBottomRow() -> UIView {
        let next = NextKeyboardButton()
        next.addTarget(self, action: #selector(nextKeyboardTapped), for: .touchUpInside)

        let space = keyButton(" ")
        space.setTitle("space", for: .normal)
        space.accessibilityLabel = "Space"

        let delete = UIButton(type: .system)
        var config = UIButton.Configuration.filled()
        config.image = UIImage(systemName: "delete.left")
        config.baseForegroundColor = .white
        config.baseBackgroundColor = KeyboardTheme.keyBackground
        config.background.cornerRadius = KeyboardTheme.radius
        delete.configuration = config
        delete.addTarget(self, action: #selector(deleteTapped), for: .touchUpInside)
        delete.accessibilityLabel = "Delete"
        delete.heightAnchor.constraint(equalToConstant: KeyboardTheme.keyHeight).isActive = true

        let row = UIStackView(arrangedSubviews: [next, space, delete])
        row.axis = .horizontal
        row.spacing = 6
        row.distribution = .fill
        next.widthAnchor.constraint(equalToConstant: 52).isActive = true
        delete.widthAnchor.constraint(equalToConstant: 56).isActive = true
        return row
    }

    private func keyButton(_ key: String) -> UIButton {
        let button = UIButton(type: .system)
        var config = UIButton.Configuration.filled()
        config.title = key
        config.baseForegroundColor = .white
        config.baseBackgroundColor = KeyboardTheme.keyBackground
        config.background.cornerRadius = KeyboardTheme.radius
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 6, bottom: 6, trailing: 6)
        button.configuration = config
        button.titleLabel?.font = .preferredFont(forTextStyle: .body)
        button.heightAnchor.constraint(equalToConstant: KeyboardTheme.keyHeight).isActive = true
        button.addAction(UIAction { [weak self] _ in
            self?.onKeyTap?(key.lowercased())
        }, for: .touchUpInside)
        button.accessibilityLabel = key == " " ? "Space" : key
        return button
    }

    private func actionButton(title: String, systemImage: String, color: UIColor) -> UIButton {
        let button = UIButton(type: .system)
        var config = UIButton.Configuration.filled()
        config.title = title
        config.image = UIImage(systemName: systemImage)
        config.imagePadding = 6
        config.baseForegroundColor = color == KeyboardTheme.accent ? KeyboardTheme.primaryButtonForeground : KeyboardTheme.secondaryButtonForeground
        config.baseBackgroundColor = color == KeyboardTheme.accent ? KeyboardTheme.primaryButtonBackground : KeyboardTheme.secondarySurface
        config.background.cornerRadius = KeyboardTheme.radius
        config.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 10, bottom: 8, trailing: 10)
        button.configuration = config
        button.titleLabel?.font = .preferredFont(forTextStyle: .subheadline)
        button.heightAnchor.constraint(equalToConstant: KeyboardTheme.actionHeight).isActive = true
        button.accessibilityLabel = title
        return button
    }

    private func makeChip(_ text: String) -> UILabel {
        let label = UILabel()
        label.text = text
        label.font = .preferredFont(forTextStyle: .caption2)
        label.adjustsFontForContentSizeCategory = true
        label.textColor = KeyboardTheme.accent
        label.backgroundColor = KeyboardTheme.accent.withAlphaComponent(0.11)
        label.layer.cornerRadius = KeyboardTheme.radius
        label.layer.masksToBounds = true
        label.textAlignment = .center
        label.widthAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
        return label
    }

    private var statusIcon: String {
        switch state {
        case .ready: return "mic.circle.fill"
        case .recording: return "waveform.circle.fill"
        case .dictatingInApp: return "arrow.up.forward.app.fill"
        case .processing: return "bolt.circle.fill"
        case .insertReady: return "text.badge.checkmark"
        case .secureCopyReady: return "doc.on.doc.fill"
        case .inserted, .copied, .savedToHistory: return "checkmark.circle.fill"
        case .staleSession, .needsFullAccess, .needsMainAppSession, .error, .unsupportedSecureField: return "exclamationmark.triangle.fill"
        default: return "keyboard"
        }
    }

    private var statusColor: UIColor {
        switch state {
        case .ready, .recording, .dictatingInApp, .processing, .insertReady, .secureCopyReady: return KeyboardTheme.accent
        case .inserted, .copied, .savedToHistory: return KeyboardTheme.success
        case .staleSession, .needsFullAccess, .needsMainAppSession, .error, .unsupportedSecureField: return KeyboardTheme.warning
        default: return KeyboardTheme.teal
        }
    }

    private var titleText: String {
        switch state {
        case .ready: return "AirNote ready"
        case .recording: return "Listening"
        case .dictatingInApp: return "Dictating in AirNote"
        case .processing: return "Processing"
        case .insertReady: return "Ready to insert"
        case .secureCopyReady: return "Copy ready"
        case .inserted: return "Inserted"
        case .staleSession: return "Session expired"
        case .needsFullAccess: return "Full Access needed"
        case .needsMainAppSession: return "Open AirNote"
        case .unsupportedSecureField: return "Secure field"
        case .error: return "AirNote needs attention"
        default: return "AirNote Keyboard"
        }
    }

    private var subtitleText: String {
        switch state {
        case .ready: return "\(SharedStore.tonePreset.capitalized) · \(SharedStore.outputLanguage.capitalized)"
        case .needsMainAppSession: return "Open AirNote and sign in to dictate."
        case .recording: return "Speak naturally. Tap stop when done."
        case .dictatingInApp: return "Speak in AirNote, then swipe back here to insert."
        case .processing(let phase): return phase
        case .insertReady: return "Review, insert, copy, or save."
        case .secureCopyReady: return "Secure field detected. Copy polished text instead."
        case .staleSession: return "Open AirNote to restart the session."
        case .needsFullAccess: return "Turn on Full Access to use voice dictation."
        case .unsupportedSecureField: return "AirNote will not insert into password, OTP, payment, or secure fields."
        case .inserted: return "Inserted into the current field."
        case .copied: return "Copied to clipboard."
        case .savedToHistory: return "Saved to AirNote history."
        case .error(let message): return message
        default: return "Manual typing still works when voice is unavailable."
        }
    }

    private var primaryActionTitle: String {
        switch state {
        case .ready: return "Start recording"
        case .recording: return "Stop"
        case .dictatingInApp: return "Open AirNote"
        case .processing: return "Working"
        case .insertReady: return "Insert"
        case .secureCopyReady: return "Copy"
        case .inserted, .copied, .savedToHistory: return "Start recording"
        case .staleSession, .needsMainAppSession: return "Open AirNote"
        case .needsFullAccess: return "Repair setup"
        case .unsupportedSecureField: return "Copy only"
        case .error: return "Retry"
        default: return "AirNote"
        }
    }

    private var primaryActionIcon: String {
        switch state {
        case .ready: return "mic.fill"
        case .recording: return "stop.fill"
        case .dictatingInApp: return "arrow.up.forward.app"
        case .insertReady: return "text.insert"
        case .secureCopyReady: return "doc.on.doc"
        case .inserted, .copied, .savedToHistory: return "mic.fill"
        case .staleSession, .needsMainAppSession: return "arrow.up.forward.app"
        case .needsFullAccess: return "wrench.and.screwdriver"
        case .unsupportedSecureField: return "doc.on.doc"
        case .error: return "arrow.clockwise"
        default: return "keyboard"
        }
    }

    private var primaryActionColor: UIColor {
        switch state {
        case .ready, .dictatingInApp, .insertReady, .secureCopyReady: return KeyboardTheme.accent
        case .recording: return KeyboardTheme.danger
        default: return .secondaryLabel
        }
    }

    private var isPrimaryActionDisabled: Bool {
        if case .processing = state {
            return true
        }
        return false
    }

    private var waveformHeights: [CGFloat] {
        switch state {
        case .recording: return [12, 24, 36, 20, 30, 18, 34, 22]
        case .processing: return [16, 16, 26, 26, 16, 16, 26, 26]
        case .insertReady, .secureCopyReady: return [14, 22, 28, 22, 14, 22, 28, 22]
        default: return [8, 12, 18, 12, 8, 12, 18, 12]
        }
    }

    private var isActiveWaveform: Bool {
        switch state {
        case .recording, .processing, .insertReady, .secureCopyReady: return true
        default: return false
        }
    }

    private var previewText: String? {
        if case .insertReady(let result) = state {
            return result.polished
        }
        if case .secureCopyReady(let result) = state {
            return result.polished
        }
        return nil
    }

    private var isCopyOnlyResult: Bool {
        if case .secureCopyReady = state {
            return true
        }
        return false
    }

    private var recoveryMessage: String? {
        switch state {
        case .needsFullAccess:
            return "Full Access is off. Manual typing still works; turn it on for cloud dictation and polish."
        case .needsMainAppSession:
            return "Open AirNote and sign in once — then dictate from the keyboard in any app."
        case .staleSession:
            return "Session expired. Restart AirNote before recording again."
        case .unsupportedSecureField:
            return "This looks like a secure field. Manual typing remains available, but AirNote will only copy final text here."
        case .error(let message):
            return message
        default:
            return nil
        }
    }

    @objc private func primaryTapped() {
        switch state {
        case .recording:
            onStop?()
        case .insertReady:
            onInsert?()
        case .secureCopyReady:
            onCopy?()
        case .staleSession, .needsMainAppSession, .needsFullAccess:
            onOpenApp?()
        case .unsupportedSecureField:
            onCopy?()
        default:
            onStart?()
        }
    }

    @objc private func insertTapped() {
        onInsert?()
    }

    @objc private func copyTapped() {
        onCopy?()
    }

    @objc private func saveTapped() {
        onSave?()
    }

    @objc private func deleteTapped() {
        onDelete?()
    }

    @objc private func nextKeyboardTapped() {
        onNextKeyboard?()
    }
}
