import UIKit
import AirNoteShared

final class KeyboardViewController: UIInputViewController {
    private var stateMachine = KeyboardStateMachine()
    private var bridge: AppGroupBridge?
    private let commandHandler = KeyboardCommandHandler()
    private let pasteboard = UIPasteboard.general
    private var refreshTimer: Timer?

    override func viewDidLoad() {
        super.viewDidLoad()
        bridge = try? AppGroupBridge()
        refreshBridgeState()
        startBridgeRefreshTimer()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        refreshBridgeState()
        startBridgeRefreshTimer()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        stopBridgeRefreshTimer()
    }

    deinit {
        stopBridgeRefreshTimer()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        refreshBridgeState()
    }

    private func refreshBridgeState() {
        let previousState = stateMachine.state
        let session = try? bridge?.read(BridgeSession.self, from: .session)
        stateMachine.apply(session: session)
        let isSecureField = TextInsertion(documentProxy: textDocumentProxy).isUnsupportedSecureField
        if let result = try? bridge?.read(BridgeResult.self, from: .result) {
            _ = stateMachine.apply(result: result, secureField: isSecureField)
        } else if isSecureField {
            stateMachine.markUnsupportedSecureField()
        }

        guard stateMachine.state != previousState else {
            return
        }
        renderCurrentState()
    }

    private func renderCurrentState() {
        view.subviews.forEach { $0.removeFromSuperview() }
        let pad = RecordingPadView(state: stateMachine.state)
        pad.translatesAutoresizingMaskIntoConstraints = false
        pad.onStart = { [weak self] in self?.startRecordingCommand() }
        pad.onStop = { [weak self] in self?.stopRecordingCommand() }
        pad.onInsert = { [weak self] in self?.insertCurrentResult() }
        pad.onCopy = { [weak self] in self?.copyCurrentResult() }
        pad.onSave = { [weak self] in self?.saveCurrentResult() }
        pad.onOpenApp = { [weak self] in self?.requestMainAppSession() }
        pad.onKeyTap = { [weak self] text in self?.textDocumentProxy.insertText(text) }
        pad.onDelete = { [weak self] in self?.textDocumentProxy.deleteBackward() }
        pad.onNextKeyboard = { [weak self] in self?.advanceToNextInputMode() }
        view.addSubview(pad)
        NSLayoutConstraint.activate([
            pad.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            pad.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            pad.topAnchor.constraint(equalTo: view.topAnchor),
            pad.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        ])
    }

    private func startRecordingCommand() {
        let context = ContextReader(documentProxy: textDocumentProxy).read()
        let command = commandHandler.makeStartRecordingCommand(context: context)
        try? bridge?.write(command, to: .command)
    }

    private func stopRecordingCommand() {
        let context = ContextReader(documentProxy: textDocumentProxy).read()
        let command = commandHandler.makeStopRecordingCommand(context: context)
        try? bridge?.write(command, to: .command)
    }

    private func requestMainAppSession() {
        let context = ContextReader(documentProxy: textDocumentProxy).read()
        let command = commandHandler.makeStartSessionCommand(context: context)
        try? bridge?.write(command, to: .command)
    }

    private func insertCurrentResult() {
        guard let result = currentResult else { return }
        if case .secureCopyReady = stateMachine.state {
            copy(result)
            return
        }

        let inserter = TextInsertion(documentProxy: textDocumentProxy)
        guard inserter.insert(result) else {
            copy(result)
            return
        }
        acknowledge(result: result, outcome: .inserted)
        stateMachine.acknowledgeInserted(resultSeq: result.resultSeq)
        renderCurrentState()
    }

    private func copyCurrentResult() {
        guard let result = currentResult else { return }
        copy(result)
    }

    private func saveCurrentResult() {
        guard let result = currentResult else { return }
        acknowledge(result: result, outcome: .savedToHistory)
        stateMachine.acknowledgeSaved(resultSeq: result.resultSeq)
        renderCurrentState()
    }

    private func startBridgeRefreshTimer() {
        guard refreshTimer == nil else { return }
        refreshTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: true) { [weak self] _ in
            self?.refreshBridgeState()
        }
    }

    private func stopBridgeRefreshTimer() {
        refreshTimer?.invalidate()
        refreshTimer = nil
    }

    private var currentResult: BridgeResult? {
        switch stateMachine.state {
        case .insertReady(let result), .secureCopyReady(let result):
            return result
        default:
            return nil
        }
    }

    private func copy(_ result: BridgeResult) {
        pasteboard.string = result.polished
        acknowledge(result: result, outcome: .copied)
        stateMachine.acknowledgeCopied(resultSeq: result.resultSeq)
        renderCurrentState()
    }

    private func acknowledge(result: BridgeResult, outcome: TerminalOutcome) {
        let ack = BridgeAck(
            resultSeq: result.resultSeq,
            sessionID: result.sessionID,
            clientRequestID: result.clientRequestID,
            outcome: outcome
        )
        try? bridge?.write(ack, to: .ack)
    }
}
