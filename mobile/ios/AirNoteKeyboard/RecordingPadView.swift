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

    // Full typing-keyboard state (letters / numbers / symbols layers, shift, delete-repeat).
    private enum ShiftState { case off, on, locked }
    private enum KeyboardLayout { case letters, symbols, moreSymbols }
    private var shiftState: ShiftState = .off
    private var layoutMode: KeyboardLayout = .letters
    private weak var keyboardStack: UIStackView?
    private var deleteRepeatTimer: Timer?
    private var lastShiftTap: Date?

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
        let minH: CGFloat = 8
        let maxH: CGFloat = 32
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

    deinit { deleteRepeatTimer?.invalidate() }

    private func build() {
        backgroundColor = KeyboardTheme.keyboardBackground

        rootStack.axis = .vertical
        rootStack.spacing = 7
        rootStack.alignment = .fill
        rootStack.translatesAutoresizingMaskIntoConstraints = false

        let surface = makeVoiceSurface()
        voiceSurface = surface
        rootStack.addArrangedSubview(surface)
        let keys = UIStackView()
        keys.axis = .vertical
        keys.spacing = 7
        keys.alignment = .fill
        keyboardStack = keys
        rootStack.addArrangedSubview(keys)
        rebuildKeys()

        addSubview(rootStack)
        NSLayoutConstraint.activate([
            rootStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            rootStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            rootStack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            rootStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8)
        ])
    }

    /// Keep the look correct when the host app flips light/dark while the keyboard
    /// is up. Adaptive UIColors update themselves; cgColor-based borders don't.
    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        guard traitCollection.userInterfaceStyle != previous?.userInterfaceStyle else { return }
        backgroundColor = KeyboardTheme.keyboardBackground
        voiceSurface?.layer.borderColor = KeyboardTheme.border.cgColor
        voiceSurface?.layer.shadowColor = KeyboardTheme.cardShadow.cgColor
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
        surface.layer.cornerRadius = KeyboardTheme.surfaceRadius
        surface.layer.cornerCurve = .continuous
        surface.layer.borderWidth = 1
        surface.layer.borderColor = KeyboardTheme.border.cgColor
        // Soft, ink-tinted elevation for depth (premium feel) — the keys stay flat below it.
        surface.layer.shadowColor = KeyboardTheme.cardShadow.cgColor
        surface.layer.shadowOpacity = 0.10
        surface.layer.shadowRadius = 16
        surface.layer.shadowOffset = CGSize(width: 0, height: 6)

        let stack = UIStackView()
        stack.axis = .vertical
        stack.spacing = 9
        stack.translatesAutoresizingMaskIntoConstraints = false

        if let preview = previewText {
            // Result: the polished text is the focus.
            stack.addArrangedSubview(makeStatusRow())
            stack.addArrangedSubview(makePreview(preview))
            stack.addArrangedSubview(makeResultActions())
        } else if let message = recoveryMessage {
            if isOpenAppExplainer {
                // A polished "open the app once" page instead of a plain error.
                stack.addArrangedSubview(makeOpenAppExplainer())
            } else {
                // Recovery: explain + a labelled repair/open button.
                stack.addArrangedSubview(makeStatusRow())
                stack.addArrangedSubview(ErrorDrawer(message: message))
                stack.addArrangedSubview(makePrimaryActions())
            }
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
            stack.leadingAnchor.constraint(equalTo: surface.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: surface.trailingAnchor, constant: -14),
            stack.topAnchor.constraint(equalTo: surface.topAnchor, constant: 14),
            stack.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -14)
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
        let size: CGFloat = 76
        let isRec: Bool = { if case .recording = state { return true } else { return false } }()
        let orb = UIButton(type: .custom)
        orb.translatesAutoresizingMaskIntoConstraints = false
        orb.layer.cornerRadius = size / 2
        orb.layer.cornerCurve = .continuous
        // Saturated vertical gradient — reads as a premium sphere vs a flat tint disc.
        let gradient = CAGradientLayer()
        gradient.frame = CGRect(x: 0, y: 0, width: size, height: size)
        gradient.colors = isRec ? KeyboardTheme.recordingGradientStops : KeyboardTheme.accentGradientStops
        gradient.startPoint = CGPoint(x: 0.5, y: 0)
        gradient.endPoint = CGPoint(x: 0.5, y: 1)
        gradient.cornerRadius = size / 2
        gradient.cornerCurve = .continuous
        orb.layer.insertSublayer(gradient, at: 0)
        // Glossy white rim + colored halo glow (the single biggest premium tell).
        orb.layer.borderWidth = 1
        orb.layer.borderColor = UIColor.white.withAlphaComponent(0.25).cgColor
        orb.layer.shadowColor = (isRec ? KeyboardTheme.danger : KeyboardTheme.accent).cgColor
        orb.layer.shadowOpacity = 0.38
        orb.layer.shadowRadius = 16
        orb.layer.shadowOffset = CGSize(width: 0, height: 4)
        orb.layer.shadowPath = UIBezierPath(ovalIn: CGRect(x: 0, y: 0, width: size, height: size)).cgPath
        var config = UIButton.Configuration.plain()
        config.image = UIImage(
            systemName: primaryActionIcon,
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 26, weight: .semibold)
        )
        config.baseForegroundColor = .white
        orb.configuration = config
        orb.isUserInteractionEnabled = !isPrimaryActionDisabled
        orb.alpha = isPrimaryActionDisabled ? 0.7 : 1.0
        orb.addTarget(self, action: #selector(primaryTapped), for: .touchUpInside)
        orb.accessibilityLabel = primaryActionTitle
        if isRec { addRecordingPulse(to: orb) }
        if isCelebration { addPop(to: orb) }

        let wrap = UIView()
        wrap.addSubview(orb)
        NSLayoutConstraint.activate([
            orb.widthAnchor.constraint(equalToConstant: size),
            orb.heightAnchor.constraint(equalToConstant: size),
            orb.centerXAnchor.constraint(equalTo: wrap.centerXAnchor),
            orb.topAnchor.constraint(equalTo: wrap.topAnchor, constant: 6),
            orb.bottomAnchor.constraint(equalTo: wrap.bottomAnchor, constant: -6)
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
        block.spacing = 3
        block.accessibilityLabel = "\(titleText), \(subtitleText)"
        return block
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
        bars.spacing = 4
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
            bars.widthAnchor.constraint(equalToConstant: 172)
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
        let label = PaddedLabel()
        label.text = " "
        label.font = .preferredFont(forTextStyle: .body)
        label.adjustsFontForContentSizeCategory = true
        // Wrap across up to 5 lines and keep the LATEST words visible (truncate the
        // head, not the tail) so a long Hinglish sentence is readable as a
        // paragraph instead of one clipped line.
        label.numberOfLines = 5
        label.lineBreakMode = .byTruncatingHead
        label.textColor = KeyboardTheme.foreground
        label.backgroundColor = KeyboardTheme.secondarySurface
        label.layer.cornerRadius = KeyboardTheme.tileRadius
        label.layer.cornerCurve = .continuous
        label.layer.masksToBounds = true
        label.accessibilityLabel = "Live transcript"
        liveLabel = label
        return label
    }

    private func makePreview(_ text: String) -> UIView {
        let label = PaddedLabel()
        label.text = text
        label.font = .preferredFont(forTextStyle: .subheadline)
        label.adjustsFontForContentSizeCategory = true
        // Show the whole polished result, wrapped, so the user can read and edit it
        // (capped so a very long result doesn't make the keyboard huge).
        label.numberOfLines = 8
        label.lineBreakMode = .byWordWrapping
        label.textColor = KeyboardTheme.foreground
        label.backgroundColor = KeyboardTheme.secondarySurface
        label.layer.cornerRadius = KeyboardTheme.tileRadius
        label.layer.cornerCurve = .continuous
        label.layer.masksToBounds = true
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

    /// True for the recovery states whose fix is simply "open AirNote".
    private var isOpenAppExplainer: Bool {
        switch state {
        case .needsMainAppSession, .staleSession, .needsFullAccess: return true
        default: return false
        }
    }

    /// A polished "open the app once" page — a gradient logo badge, a friendly
    /// headline + explanation, and a prominent Open button — shown instead of the
    /// plain error drawer for the states where the fix is just to open AirNote.
    private func makeOpenAppExplainer() -> UIView {
        let badgeSize: CGFloat = 54
        let badge = UIView()
        badge.translatesAutoresizingMaskIntoConstraints = false
        badge.layer.cornerRadius = 16
        badge.layer.cornerCurve = .continuous
        let grad = CAGradientLayer()
        grad.frame = CGRect(x: 0, y: 0, width: badgeSize, height: badgeSize)
        grad.colors = KeyboardTheme.accentGradientStops
        grad.startPoint = CGPoint(x: 0.5, y: 0)
        grad.endPoint = CGPoint(x: 0.5, y: 1)
        grad.cornerRadius = 16
        grad.cornerCurve = .continuous
        badge.layer.insertSublayer(grad, at: 0)
        badge.layer.shadowColor = KeyboardTheme.accent.cgColor
        badge.layer.shadowOpacity = 0.35
        badge.layer.shadowRadius = 12
        badge.layer.shadowOffset = CGSize(width: 0, height: 4)
        badge.layer.shadowPath = UIBezierPath(roundedRect: CGRect(x: 0, y: 0, width: badgeSize, height: badgeSize), cornerRadius: 16).cgPath
        let glyph = UIImageView(image: UIImage(
            systemName: "waveform",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 24, weight: .bold)
        ))
        glyph.tintColor = .white
        glyph.translatesAutoresizingMaskIntoConstraints = false
        badge.addSubview(glyph)
        NSLayoutConstraint.activate([
            badge.widthAnchor.constraint(equalToConstant: badgeSize),
            badge.heightAnchor.constraint(equalToConstant: badgeSize),
            glyph.centerXAnchor.constraint(equalTo: badge.centerXAnchor),
            glyph.centerYAnchor.constraint(equalTo: badge.centerYAnchor),
        ])

        let title = UILabel()
        title.font = .preferredFont(forTextStyle: .headline)
        title.adjustsFontForContentSizeCategory = true
        title.textColor = KeyboardTheme.foreground
        title.text = explainerTitle
        title.textAlignment = .center
        title.numberOfLines = 2

        let subtitle = UILabel()
        subtitle.font = .preferredFont(forTextStyle: .subheadline)
        subtitle.adjustsFontForContentSizeCategory = true
        subtitle.textColor = KeyboardTheme.muted
        subtitle.text = explainerSubtitle
        subtitle.textAlignment = .center
        subtitle.numberOfLines = 4

        let open = actionButton(title: "Open AirNote", systemImage: "arrow.up.forward.app.fill", color: KeyboardTheme.accent)
        open.addTarget(self, action: #selector(primaryTapped), for: .touchUpInside)
        open.accessibilityHint = "Opens the AirNote app"
        let openRow = UIStackView(arrangedSubviews: [open])
        openRow.axis = .horizontal
        openRow.alignment = .fill

        let stack = UIStackView(arrangedSubviews: [badge, title, subtitle, openRow])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 9
        stack.setCustomSpacing(14, after: subtitle)
        openRow.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        stack.accessibilityLabel = "\(explainerTitle). \(explainerSubtitle)"
        return stack
    }

    private var explainerTitle: String {
        switch state {
        case .needsMainAppSession: return "Open AirNote once"
        case .staleSession: return "Wake AirNote up"
        case .needsFullAccess: return "Turn on Full Access"
        default: return "Open AirNote"
        }
    }

    private var explainerSubtitle: String {
        switch state {
        case .needsMainAppSession:
            return "Open the app and sign in once. Then come back here and dictate in any app — no need to reopen it."
        case .staleSession:
            return "Your session went to sleep. Tap to reopen AirNote, then dictate from here again."
        case .needsFullAccess:
            return "AirNote needs Full Access for cloud dictation. Manual typing keeps working in the meantime."
        default:
            return "Tap to open AirNote."
        }
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

    // MARK: - Full typing keyboard (letters / numbers / symbols, shift, delete-repeat)

    private func rebuildKeys() {
        guard let keys = keyboardStack else { return }
        keys.arrangedSubviews.forEach { $0.removeFromSuperview() }
        keys.addArrangedSubview(makeKeyboardHandle())
        // Collapsed = compact voice mode: just the handle (frees screen space; tap
        // the handle to bring the keyboard up to edit, then collapse it again).
        guard !SharedStore.keyboardKeysCollapsed else { return }
        let rows: [[String]]
        switch layoutMode {
        case .letters:
            rows = [
                ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"],
                ["a", "s", "d", "f", "g", "h", "j", "k", "l"],
                ["z", "x", "c", "v", "b", "n", "m"]
            ]
        case .symbols:
            rows = [
                ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
                ["-", "/", ":", ";", "(", ")", "$", "&", "@", "\""],
                [".", ",", "?", "!", "'"]
            ]
        case .moreSymbols:
            rows = [
                ["[", "]", "{", "}", "#", "%", "^", "*", "+", "="],
                ["_", "\\", "|", "~", "<", ">", "€", "£", "¥", "•"],
                [".", ",", "?", "!", "'"]
            ]
        }
        keys.addArrangedSubview(charRow(rows[0]))
        keys.addArrangedSubview(charRow(rows[1], inset: layoutMode == .letters ? 18 : 0))
        keys.addArrangedSubview(thirdRow(rows[2]))
        keys.addArrangedSubview(bottomRow())
    }

    private func charRow(_ chars: [String], inset: CGFloat = 0) -> UIView {
        let inner = UIStackView()
        inner.axis = .horizontal
        inner.spacing = 5
        inner.distribution = .fillEqually
        chars.forEach { inner.addArrangedSubview(charKey($0)) }
        guard inset > 0 else { return inner }
        let row = UIStackView(arrangedSubviews: [fixedSpacer(inset / 2), inner, fixedSpacer(inset / 2)])
        row.axis = .horizontal
        row.spacing = 5
        row.distribution = .fill
        return row
    }

    private func thirdRow(_ chars: [String]) -> UIView {
        let leading: UIButton
        if layoutMode == .letters {
            leading = shiftKey()
        } else {
            leading = layoutToggleKey(layoutMode == .symbols ? "#+=" : "123",
                                      to: layoutMode == .symbols ? .moreSymbols : .symbols)
        }
        let inner = UIStackView()
        inner.axis = .horizontal
        inner.spacing = 5
        inner.distribution = .fillEqually
        chars.forEach { inner.addArrangedSubview(charKey($0)) }
        let del = deleteKey()
        let row = UIStackView(arrangedSubviews: [leading, inner, del])
        row.axis = .horizontal
        row.spacing = 5
        row.distribution = .fill
        leading.widthAnchor.constraint(equalToConstant: 46).isActive = true
        del.widthAnchor.constraint(equalToConstant: 46).isActive = true
        return row
    }

    private func bottomRow() -> UIView {
        let modeKey = layoutToggleKey(layoutMode == .letters ? "123" : "ABC",
                                      to: layoutMode == .letters ? .symbols : .letters)
        let globe = NextKeyboardButton()
        globe.addTarget(self, action: #selector(nextKeyboardTapped), for: .touchUpInside)
        globe.heightAnchor.constraint(equalToConstant: KeyboardTheme.keyHeight).isActive = true
        let space = spaceKey()
        let ret = specialKey(title: "return")
        ret.addAction(UIAction { [weak self] _ in self?.onKeyTap?("\n") }, for: .touchUpInside)
        let row = UIStackView(arrangedSubviews: [modeKey, globe, space, ret])
        row.axis = .horizontal
        row.spacing = 6
        row.distribution = .fill
        modeKey.widthAnchor.constraint(equalToConstant: 46).isActive = true
        globe.widthAnchor.constraint(equalToConstant: 46).isActive = true
        ret.widthAnchor.constraint(equalToConstant: 96).isActive = true
        return row
    }

    private func fixedSpacer(_ width: CGFloat) -> UIView {
        let v = UIView()
        v.widthAnchor.constraint(equalToConstant: width).isActive = true
        return v
    }

    /// A slim handle that collapses/expands the typing keys, so the keyboard can
    /// shrink to just the voice surface while dictating and pop back up to edit.
    private func makeKeyboardHandle() -> UIView {
        let collapsed = SharedStore.keyboardKeysCollapsed
        // Collapsed: a prominent accent pill ("Show keyboard") so it's obvious the
        // QWERTY can come back. Expanded: a subtle "Hide keyboard" grabber — a tap
        // either way folds the keys to the voice surface (WhisperFlow-style).
        let tint = collapsed ? KeyboardTheme.accent : KeyboardTheme.muted
        let button = UIButton(type: .system)
        var config = UIButton.Configuration.plain()
        config.image = UIImage(
            systemName: collapsed ? "keyboard.chevron.compact.up" : "chevron.compact.down",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .bold)
        )
        config.title = collapsed ? "Show keyboard" : "Hide keyboard"
        config.imagePadding = 6
        config.baseForegroundColor = tint
        config.background.backgroundColor = tint.withAlphaComponent(collapsed ? 0.14 : 0.08)
        config.background.cornerRadius = 16
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 16)
        button.configuration = config
        button.titleLabel?.font = .systemFont(ofSize: 13, weight: .semibold)
        button.addAction(UIAction { [weak self] _ in self?.toggleKeysCollapsed() }, for: .touchUpInside)
        button.accessibilityLabel = collapsed ? "Show keyboard" : "Hide keyboard"
        let buttonRow = UIStackView(arrangedSubviews: [UIView(), button, UIView()])
        buttonRow.axis = .horizontal
        buttonRow.distribution = .fill

        // A small grabber bar above the pill — reads as a draggable sheet handle.
        let grabber = UIView()
        grabber.backgroundColor = KeyboardTheme.muted.withAlphaComponent(0.35)
        grabber.layer.cornerRadius = 2
        grabber.translatesAutoresizingMaskIntoConstraints = false
        grabber.widthAnchor.constraint(equalToConstant: 36).isActive = true
        grabber.heightAnchor.constraint(equalToConstant: 4).isActive = true
        let grabberRow = UIStackView(arrangedSubviews: [UIView(), grabber, UIView()])
        grabberRow.axis = .horizontal
        grabberRow.distribution = .fill

        let column = UIStackView(arrangedSubviews: [grabberRow, buttonRow])
        column.axis = .vertical
        column.spacing = 5
        return column
    }

    private func toggleKeysCollapsed() {
        SharedStore.keyboardKeysCollapsed.toggle()
        UIImpactFeedbackGenerator(style: .soft).impactOccurred()
        // Only the typing keys rebuild; the voice surface (and any active waveform
        // constraints) is left untouched, so folding never disturbs dictation.
        rebuildKeys()
        UIView.animate(
            withDuration: 0.34, delay: 0,
            usingSpringWithDamping: 0.82, initialSpringVelocity: 0.4,
            options: [.curveEaseOut, .allowUserInteraction]
        ) { self.layoutIfNeeded() }
    }

    /// Character key — letters follow the shift state; everything else inserts literally.
    private func charKey(_ ch: String) -> UIButton {
        let isLetter = ch.count == 1 && ch.rangeOfCharacter(from: .letters) != nil
        let display = (isLetter && shiftState != .off) ? ch.uppercased() : ch
        let button = baseKey(title: display, background: KeyboardTheme.keyBackground, fontSize: 22)
        button.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.onKeyTap?(display)
            if isLetter, self.shiftState == .on {
                self.shiftState = .off
                self.rebuildKeys()
            }
        }, for: .touchUpInside)
        button.accessibilityLabel = display
        return button
    }

    private func spaceKey() -> UIButton {
        let button = baseKey(title: "space", background: KeyboardTheme.keyBackground, fontSize: 15)
        button.addAction(UIAction { [weak self] _ in self?.onKeyTap?(" ") }, for: .touchUpInside)
        button.accessibilityLabel = "Space"
        return button
    }

    private func shiftKey() -> UIButton {
        let icon: String
        switch shiftState {
        case .off: icon = "shift"
        case .on: icon = "shift.fill"
        case .locked: icon = "capslock.fill"
        }
        let button = baseKey(title: nil, systemImage: icon, background: KeyboardTheme.secondarySurface, fontSize: 18)
        button.addAction(UIAction { [weak self] _ in self?.shiftTapped() }, for: .touchUpInside)
        button.accessibilityLabel = "Shift"
        return button
    }

    private func shiftTapped() {
        let now = Date()
        switch shiftState {
        case .off:
            if let last = lastShiftTap, now.timeIntervalSince(last) < 0.3 {
                shiftState = .locked
            } else {
                shiftState = .on
            }
        case .on, .locked:
            shiftState = .off
        }
        lastShiftTap = now
        rebuildKeys()
    }

    private func layoutToggleKey(_ title: String, to mode: KeyboardLayout) -> UIButton {
        let button = baseKey(title: title, background: KeyboardTheme.secondarySurface, fontSize: 16, weight: .medium)
        button.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.layoutMode = mode
            if mode != .letters { self.shiftState = .off }
            self.rebuildKeys()
        }, for: .touchUpInside)
        button.accessibilityLabel = title
        return button
    }

    private func deleteKey() -> UIButton {
        let button = baseKey(title: nil, systemImage: "delete.left", background: KeyboardTheme.secondarySurface, fontSize: 18)
        button.addTarget(self, action: #selector(deleteTouchDown), for: .touchDown)
        button.addTarget(self, action: #selector(deleteTouchUp), for: [.touchUpInside, .touchUpOutside, .touchCancel])
        button.accessibilityLabel = "Delete"
        return button
    }

    @objc private func deleteTouchDown() {
        onDelete?()
        deleteRepeatTimer?.invalidate()
        deleteRepeatTimer = Timer.scheduledTimer(withTimeInterval: 0.11, repeats: true) { [weak self] _ in
            self?.onDelete?()
        }
    }

    @objc private func deleteTouchUp() {
        deleteRepeatTimer?.invalidate()
        deleteRepeatTimer = nil
    }

    /// Stop any in-flight delete-repeat (called when the keyboard disappears or
    /// deallocates, so it can never fire on a detached text proxy).
    func stopDeleteTimer() {
        deleteRepeatTimer?.invalidate()
        deleteRepeatTimer = nil
    }

    private func specialKey(title: String) -> UIButton {
        baseKey(title: title, background: KeyboardTheme.secondarySurface, fontSize: 16, weight: .medium)
    }

    private func baseKey(title: String?, systemImage: String? = nil, background: UIColor, fontSize: CGFloat, weight: UIFont.Weight = .regular) -> UIButton {
        let button = UIButton(type: .system)
        var config = UIButton.Configuration.filled()
        config.title = title
        if let systemImage { config.image = UIImage(systemName: systemImage) }
        config.baseForegroundColor = KeyboardTheme.foreground
        config.baseBackgroundColor = background
        config.background.cornerRadius = KeyboardTheme.radius
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 4, bottom: 6, trailing: 4)
        button.configuration = config
        if title != nil {
            button.titleLabel?.font = .systemFont(ofSize: fontSize, weight: weight)
        }
        button.heightAnchor.constraint(equalToConstant: KeyboardTheme.keyHeight).isActive = true
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
        case .recording: return [10, 18, 28, 36, 24, 32, 20, 34, 22, 30, 16, 26, 12]
        case .processing, .teaching: return [14, 18, 24, 28, 22, 16, 26, 20, 28, 22, 16, 24, 18]
        case .insertReady, .secureCopyReady: return [12, 18, 24, 28, 22, 16, 26, 20, 24, 18, 14, 22, 16]
        default: return [7, 10, 14, 18, 12, 9, 16, 11, 15, 10, 8, 13, 9]
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

/// A UILabel that actually insets its text — UILabel ignores directionalLayoutMargins
/// for text, which is why the transcript/preview looked cramped against the rounded
/// surface. This pads the text inside the background on all four sides and sizes
/// itself (and wraps) correctly.
private final class PaddedLabel: UILabel {
    var insets = UIEdgeInsets(top: 8, left: 10, bottom: 8, right: 10)

    override func drawText(in rect: CGRect) {
        super.drawText(in: rect.inset(by: insets))
    }

    override var intrinsicContentSize: CGSize {
        let base = super.intrinsicContentSize
        return CGSize(width: base.width + insets.left + insets.right,
                      height: base.height + insets.top + insets.bottom)
    }

    override func textRect(forBounds bounds: CGRect, limitedToNumberOfLines numberOfLines: Int) -> CGRect {
        var rect = super.textRect(forBounds: bounds.inset(by: insets), limitedToNumberOfLines: numberOfLines)
        rect.origin.x -= insets.left
        rect.origin.y -= insets.top
        rect.size.width += insets.left + insets.right
        rect.size.height += insets.top + insets.bottom
        return rect
    }
}
