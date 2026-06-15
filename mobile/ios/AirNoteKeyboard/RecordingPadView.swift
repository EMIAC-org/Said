import UIKit
import AirNoteShared

final class RecordingPadView: UIView {
    var onStart: (() -> Void)?
    var onStop: (() -> Void)?
    var onInsert: (() -> Void)?
    var onCopy: (() -> Void)?
    var onSave: (() -> Void)?
    var onTeachFix: (() -> Void)?
    var onOpenApp: (() -> Void)?
    var onKeyTap: ((String) -> Void)?
    var onDelete: (() -> Void)?
    var onNextKeyboard: (() -> Void)?

    private var state: KeyboardState
    private var canTeachFix: Bool

    private let rootStack = UIStackView()
    private var voiceSurface: UIView?
    private weak var liveLabel: UILabel?
    /// Height constraints of the live waveform bars, driven by the real mic level
    /// while recording (see setAudioLevel). Empty in non-recording states.
    private var waveBarHeights: [NSLayoutConstraint] = []
    private var waveLevels: [CGFloat] = []

    /// Update the live transcript shown while recording, without a full re-render.
    func setLivePartial(_ text: String) {
        liveLabel?.text = text.isEmpty ? " " : text
    }

    /// Drive the live waveform from the real mic level (0...1) while recording.
    /// Pushes the newest sample in on the right and scrolls older samples left so
    /// the bars track the user's voice instead of a canned animation loop.
    func setAudioLevel(_ level: Float) {
        guard !waveBarHeights.isEmpty else { return }
        let lvl = CGFloat(min(1, max(0, level)))
        if waveLevels.count == waveBarHeights.count {
            waveLevels.removeFirst()
            waveLevels.append(lvl)
        } else {
            waveLevels = Array(repeating: lvl, count: waveBarHeights.count)
        }
        let minH: CGFloat = 6
        let maxH: CGFloat = 26
        for (index, constraint) in waveBarHeights.enumerated() {
            let sample = index < waveLevels.count ? waveLevels[index] : lvl
            // Gamma < 1 lifts quiet speech so the bars still move at low volume.
            let shaped = pow(sample, 0.7)
            constraint.constant = minH + shaped * (maxH - minH)
        }
        UIView.animate(withDuration: 0.07, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) {
            self.layoutIfNeeded()
        }
    }

    init(state: KeyboardState, canTeachFix: Bool = false) {
        self.state = state
        self.canTeachFix = canTeachFix
        super.init(frame: .zero)
        build()
    }

    required init?(coder: NSCoder) {
        self.state = .notConfigured
        self.canTeachFix = false
        super.init(coder: coder)
        build()
    }

    private func build() {
        backgroundColor = KeyboardTheme.keyboardBackground

        rootStack.axis = .vertical
        rootStack.spacing = 7
        rootStack.alignment = .fill
        rootStack.translatesAutoresizingMaskIntoConstraints = false

        let surface = makeVoiceSurface()
        voiceSurface = surface
        rootStack.addArrangedSubview(surface)
        makeKeyboardRows().forEach { rootStack.addArrangedSubview($0) }
        rootStack.addArrangedSubview(makeBottomRow())

        addSubview(rootStack)
        NSLayoutConstraint.activate([
            rootStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            rootStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            rootStack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            rootStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8)
        ])
    }

    /// Swap ONLY the voice surface for the new state — the keys stay put, so
    /// state changes never flash the whole keyboard. The surface settles in with
    /// a soft spring (Wispr-style). Returns without animating identical states.
    func update(state: KeyboardState, canTeachFix: Bool, animated: Bool) {
        self.state = state
        self.canTeachFix = canTeachFix
        // Drop references to the previous surface's waveform constraints so
        // setAudioLevel can never mutate detached views.
        waveBarHeights.removeAll()
        waveLevels.removeAll()

        let newSurface = makeVoiceSurface()
        if let old = voiceSurface {
            rootStack.removeArrangedSubview(old)
            old.removeFromSuperview()
        }
        rootStack.insertArrangedSubview(newSurface, at: 0)
        voiceSurface = newSurface

        guard animated else { return }
        newSurface.alpha = 0
        newSurface.transform = CGAffineTransform(translationX: 0, y: -7)
        UIView.animate(
            withDuration: 0.26,
            delay: 0,
            usingSpringWithDamping: 0.82,
            initialSpringVelocity: 0.5,
            options: [.curveEaseOut, .allowUserInteraction]
        ) {
            newSurface.alpha = 1
            newSurface.transform = .identity
        }
    }

    private func makeVoiceSurface() -> UIView {
        let surface = UIView()
        surface.backgroundColor = KeyboardTheme.surfaceBackground
        surface.layer.cornerRadius = KeyboardTheme.radius
        surface.layer.borderWidth = 1
        surface.layer.borderColor = KeyboardTheme.border.cgColor
        // Soft elevation for depth (premium feel) — the keys stay flat below it.
        surface.layer.shadowColor = UIColor.black.cgColor
        surface.layer.shadowOpacity = 0.12
        surface.layer.shadowRadius = 7
        surface.layer.shadowOffset = CGSize(width: 0, height: 2)

        let stack = UIStackView()
        stack.axis = .vertical
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false

        if let preview = previewText {
            // Result: the polished text is the focus.
            stack.addArrangedSubview(makeStatusRow())
            stack.addArrangedSubview(makePreview(preview))
            stack.addArrangedSubview(makeResultActions())
        } else if let message = recoveryMessage {
            // Recovery: explain + a labelled repair/open button.
            stack.addArrangedSubview(makeStatusRow())
            stack.addArrangedSubview(ErrorDrawer(message: message))
            stack.addArrangedSubview(makePrimaryActions())
        } else {
            // Hero: a single Mic Orb IS the primary action (tap to start/stop),
            // with a compact mode pill above and the title/waveform/live words
            // below — Wispr-style, replacing the dense status row + 3-button triage.
            stack.addArrangedSubview(makePill())
            stack.addArrangedSubview(makeOrb())
            stack.addArrangedSubview(makeTitleBlock())
            if isActiveWaveform { stack.addArrangedSubview(makeWaveform()) }
            if case .recording = state { stack.addArrangedSubview(makeLiveTranscript()) }
            if canTeachFix, isPostInsertState { stack.addArrangedSubview(makePostInsertActions()) }
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

    // MARK: - Wispr-style hero (Mic Orb + pill + title)

    /// A single compact mode·language pill, trailing-aligned — replaces the two
    /// always-on chips in the hero states.
    private func makePill() -> UIView {
        let pill = makeChip("\(SharedStore.tonePreset.capitalized) · \(SharedStore.outputLanguage.capitalized)")
        pill.accessibilityLabel = "Mode: \(SharedStore.tonePreset.capitalized), \(SharedStore.outputLanguage.capitalized)"
        let row = UIStackView(arrangedSubviews: [UIView(), pill])
        row.axis = .horizontal
        return row
    }

    /// The hero: a large circular Mic Orb that IS the primary action (tap to
    /// start/stop/open per state). Breathes while recording, pops on success.
    private func makeOrb() -> UIView {
        let size: CGFloat = 60
        let orb = UIButton(type: .custom)
        orb.translatesAutoresizingMaskIntoConstraints = false
        orb.backgroundColor = orbFill
        orb.layer.cornerRadius = size / 2
        orb.layer.borderWidth = 2
        orb.layer.borderColor = orbRing.cgColor
        var config = UIButton.Configuration.plain()
        config.image = UIImage(
            systemName: primaryActionIcon,
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 22, weight: .semibold)
        )
        config.baseForegroundColor = orbTint
        orb.configuration = config
        orb.isUserInteractionEnabled = !isPrimaryActionDisabled
        orb.alpha = isPrimaryActionDisabled ? 0.7 : 1.0
        orb.addTarget(self, action: #selector(primaryTapped), for: .touchUpInside)
        orb.accessibilityLabel = primaryActionTitle
        if case .recording = state { addRecordingPulse(to: orb) }
        if isCelebration { addPop(to: orb) }

        let wrap = UIView()
        wrap.addSubview(orb)
        NSLayoutConstraint.activate([
            orb.widthAnchor.constraint(equalToConstant: size),
            orb.heightAnchor.constraint(equalToConstant: size),
            orb.centerXAnchor.constraint(equalTo: wrap.centerXAnchor),
            orb.topAnchor.constraint(equalTo: wrap.topAnchor, constant: 2),
            orb.bottomAnchor.constraint(equalTo: wrap.bottomAnchor, constant: -2)
        ])
        return wrap
    }

    /// Centered title + subtitle under the orb.
    private func makeTitleBlock() -> UIView {
        let title = UILabel()
        title.font = .preferredFont(forTextStyle: .headline)
        title.adjustsFontForContentSizeCategory = true
        title.textColor = KeyboardTheme.foreground
        title.text = titleText
        title.textAlignment = .center

        let subtitle = UILabel()
        subtitle.font = .preferredFont(forTextStyle: .caption1)
        subtitle.adjustsFontForContentSizeCategory = true
        subtitle.textColor = KeyboardTheme.muted
        subtitle.text = subtitleText
        subtitle.textAlignment = .center
        subtitle.numberOfLines = 2

        let block = UIStackView(arrangedSubviews: [title, subtitle])
        block.axis = .vertical
        block.alignment = .center
        block.spacing = 1
        block.accessibilityLabel = "\(titleText), \(subtitleText)"
        return block
    }

    /// Orb fill — danger tint while recording, otherwise the state's accent/success.
    private var orbFill: UIColor {
        if case .recording = state { return KeyboardTheme.danger.withAlphaComponent(0.16) }
        return statusColor.withAlphaComponent(0.14)
    }

    private var orbRing: UIColor {
        if case .recording = state { return KeyboardTheme.danger.withAlphaComponent(0.6) }
        return statusColor.withAlphaComponent(0.55)
    }

    private var orbTint: UIColor {
        if case .recording = state { return KeyboardTheme.danger }
        return statusColor
    }

    private func makeStatusRow() -> UIView {
        let icon = UIImageView(image: UIImage(systemName: statusIcon))
        icon.tintColor = statusColor
        icon.contentMode = .scaleAspectFit
        icon.setContentHuggingPriority(.required, for: .horizontal)
        icon.widthAnchor.constraint(equalToConstant: 24).isActive = true
        if isCelebration { addPop(to: icon) }

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

        let bars = UIStackView()
        bars.axis = .horizontal
        bars.alignment = .center
        bars.distribution = .equalCentering
        bars.spacing = 5
        bars.translatesAutoresizingMaskIntoConstraints = false

        // While recording, the bars are driven by the REAL mic level (setAudioLevel)
        // for a live waveform. In other active states (processing/teaching) there is
        // no live audio, so fall back to the canned "breathing" animation.
        waveBarHeights.removeAll()
        let isLive = (state == .recording)
        if isLive { waveLevels = Array(repeating: 0.12, count: waveformHeights.count) }

        for (index, height) in waveformHeights.enumerated() {
            let bar = UIView()
            bar.backgroundColor = statusColor.withAlphaComponent(isActiveWaveform ? 0.95 : 0.45)
            bar.layer.cornerRadius = 3
            bar.widthAnchor.constraint(equalToConstant: 6).isActive = true
            let heightConstraint = bar.heightAnchor.constraint(equalToConstant: isLive ? 6 : height)
            heightConstraint.isActive = true
            if isLive {
                waveBarHeights.append(heightConstraint)
            } else if isActiveWaveform {
                addWaveAnimation(to: bar, index: index)
            }
            bars.addArrangedSubview(bar)
        }

        container.addSubview(bars)
        NSLayoutConstraint.activate([
            bars.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            bars.topAnchor.constraint(equalTo: container.topAnchor, constant: 2),
            bars.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -2),
            bars.widthAnchor.constraint(equalToConstant: 132)
        ])
        container.accessibilityLabel = "Audio level"
        return container
    }

    private var isCelebration: Bool {
        switch state {
        case .inserted, .copied, .savedToHistory, .learned: return true
        default: return false
        }
    }

    /// A spring "pop" on the success checkmark so an insert/teach feels rewarding.
    private func addPop(to view: UIView) {
        let pop = CASpringAnimation(keyPath: "transform.scale")
        pop.fromValue = 0.45
        pop.toValue = 1.0
        pop.damping = 9
        pop.stiffness = 190
        pop.mass = 0.7
        pop.initialVelocity = 2.0
        pop.duration = pop.settlingDuration
        view.layer.add(pop, forKey: "pop")
    }

    /// A continuous, staggered "breathing" scale on each waveform bar so the
    /// surface feels alive while recording/processing — a traveling wave, like
    /// Wispr. Phase is offset per bar via timeOffset (no wall-clock dependency).
    private func addWaveAnimation(to bar: UIView, index: Int) {
        let pulse = CABasicAnimation(keyPath: "transform.scale.y")
        pulse.fromValue = 0.45
        pulse.toValue = 1.0
        pulse.duration = 0.55
        pulse.autoreverses = true
        pulse.repeatCount = .infinity
        pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        pulse.timeOffset = Double(index) * 0.11
        bar.layer.add(pulse, forKey: "wave")
    }

    /// Live transcript shown while recording — words appear as the user speaks
    /// (the warm app streams romanized partials over the App Group).
    private func makeLiveTranscript() -> UIView {
        let label = UILabel()
        label.text = " "
        label.font = .preferredFont(forTextStyle: .body)
        label.adjustsFontForContentSizeCategory = true
        label.numberOfLines = 2
        label.textColor = KeyboardTheme.foreground
        label.backgroundColor = KeyboardTheme.secondarySurface
        label.layer.cornerRadius = KeyboardTheme.radius
        label.layer.masksToBounds = true
        label.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 8, leading: 10, bottom: 8, trailing: 10)
        label.accessibilityLabel = "Live transcript"
        liveLabel = label
        return label
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
        if case .recording = state { addRecordingPulse(to: primary) }

        let row = UIStackView(arrangedSubviews: [primary])
        row.axis = .horizontal
        row.spacing = 8
        row.alignment = .fill
        return row
    }

    /// A slow, soft breathing scale on the Stop button while recording — signals
    /// "live" without distracting from the text being dictated.
    private func addRecordingPulse(to button: UIView) {
        let pulse = CABasicAnimation(keyPath: "transform.scale")
        pulse.fromValue = 1.0
        pulse.toValue = 1.035
        pulse.duration = 0.75
        pulse.autoreverses = true
        pulse.repeatCount = .infinity
        pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        button.layer.add(pulse, forKey: "recordingPulse")
    }

    /// Post-insertion: re-record OR teach a correction made in-place.
    private func makePostInsertActions() -> UIView {
        let primary = actionButton(title: primaryActionTitle, systemImage: primaryActionIcon, color: primaryActionColor)
        primary.addTarget(self, action: #selector(primaryTapped), for: .touchUpInside)

        let teach = actionButton(title: "Teach a fix", systemImage: "checkmark.seal", color: .secondaryLabel)
        teach.addTarget(self, action: #selector(teachTapped), for: .touchUpInside)
        teach.accessibilityHint = "If you fixed a word above, teach AirNote the correction"

        let row = UIStackView(arrangedSubviews: [primary, teach])
        row.axis = .horizontal
        row.spacing = 8
        row.distribution = .fillEqually
        return row
    }

    private var isPostInsertState: Bool {
        switch state {
        case .inserted, .copied, .savedToHistory, .learned: return true
        default: return false
        }
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
        config.baseForegroundColor = KeyboardTheme.foreground
        config.baseBackgroundColor = KeyboardTheme.secondarySurface
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
        config.baseForegroundColor = KeyboardTheme.foreground   // adaptive — was always-white (invisible in light mode)
        config.baseBackgroundColor = KeyboardTheme.keyBackground
        config.background.cornerRadius = KeyboardTheme.radius
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 6, bottom: 6, trailing: 6)
        button.configuration = config
        button.titleLabel?.font = .systemFont(ofSize: 22, weight: .regular)
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

    private func makeChip(_ text: String) -> UIView {
        let label = UILabel()
        label.text = text
        label.font = .preferredFont(forTextStyle: .caption2)
        label.adjustsFontForContentSizeCategory = true
        label.textColor = KeyboardTheme.accent
        label.textAlignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false

        // Padded capsule so glyphs never touch the rounded corners.
        let chip = UIView()
        chip.backgroundColor = KeyboardTheme.accent.withAlphaComponent(0.12)
        chip.layer.cornerRadius = 11
        chip.layer.masksToBounds = true
        chip.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: chip.leadingAnchor, constant: 9),
            label.trailingAnchor.constraint(equalTo: chip.trailingAnchor, constant: -9),
            label.topAnchor.constraint(equalTo: chip.topAnchor, constant: 3),
            label.bottomAnchor.constraint(equalTo: chip.bottomAnchor, constant: -3)
        ])
        chip.accessibilityLabel = text
        return chip
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
        case .teaching: return "bolt.circle.fill"
        case .learned: return "checkmark.seal.fill"
        case .staleSession, .needsFullAccess, .needsMainAppSession, .error, .unsupportedSecureField: return "exclamationmark.triangle.fill"
        default: return "keyboard"
        }
    }

    private var statusColor: UIColor {
        switch state {
        case .ready, .recording, .dictatingInApp, .processing, .insertReady, .secureCopyReady, .teaching: return KeyboardTheme.accent
        case .inserted, .copied, .savedToHistory, .learned: return KeyboardTheme.success
        case .staleSession, .needsFullAccess, .needsMainAppSession, .error, .unsupportedSecureField: return KeyboardTheme.warning
        default: return KeyboardTheme.teal
        }
    }

    private var titleText: String {
        switch state {
        case .ready: return "AirNote ready"
        case .recording: return "Listening"
        case .dictatingInApp: return "Open AirNote to dictate"
        case .processing: return "Processing"
        case .insertReady: return "Ready to insert"
        case .secureCopyReady: return "Copy ready"
        case .inserted: return "Inserted"
        case .teaching: return "Teaching AirNote"
        case .learned: return "Got it"
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
        case .dictatingInApp: return "Open the AirNote app — it starts recording. Speak, then come back here. (After once, you can dictate right here.)"
        case .processing(let phase): return phase
        case .insertReady: return "Review, insert, copy, or save."
        case .secureCopyReady: return "Secure field detected. Copy polished text instead."
        case .staleSession: return "Open AirNote to restart the session."
        case .needsFullAccess: return "Turn on Full Access to use voice dictation."
        case .unsupportedSecureField: return "AirNote will not insert into password, OTP, payment, or secure fields."
        case .inserted: return "Inserted. Fixed a word? Tap Teach a fix."
        case .copied: return "Copied to clipboard."
        case .savedToHistory: return "Saved to AirNote history."
        case .teaching: return "Learning your correction…"
        case .learned(let message): return message
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
        case .inserted, .copied, .savedToHistory, .learned: return "Start recording"
        case .teaching: return "Working"
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
        case .inserted, .copied, .savedToHistory, .learned: return "mic.fill"
        case .teaching: return "bolt"
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
        switch state {
        case .processing, .teaching: return true
        default: return false
        }
    }

    private var waveformHeights: [CGFloat] {
        switch state {
        case .recording: return [12, 24, 36, 20, 30, 18, 34, 22]
        case .processing, .teaching: return [16, 16, 26, 26, 16, 16, 26, 26]
        case .insertReady, .secureCopyReady: return [14, 22, 28, 22, 14, 22, 28, 22]
        default: return [8, 12, 18, 12, 8, 12, 18, 12]
        }
    }

    private var isActiveWaveform: Bool {
        switch state {
        case .recording, .processing, .insertReady, .secureCopyReady, .teaching: return true
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

    @objc private func teachTapped() {
        onTeachFix?()
    }

    @objc private func deleteTapped() {
        onDelete?()
    }

    @objc private func nextKeyboardTapped() {
        onNextKeyboard?()
    }
}
