import UIKit
import AirNoteShared

final class KeyboardViewController: UIInputViewController {
    private var stateMachine = KeyboardStateMachine()
    private var bridge: AppGroupBridge?
    private let commandHandler = KeyboardCommandHandler()
    private let pasteboard = UIPasteboard.general

    override func viewDidLoad() {
        super.viewDidLoad()
        bridge = try? AppGroupBridge()
        renderCurrentState()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        refreshBridgeState()
    }

    private func refreshBridgeState() {
        let session = try? bridge?.read(BridgeSession.self, from: .session)
        stateMachine.apply(session: session)
        if TextInsertion(documentProxy: textDocumentProxy).isUnsupportedSecureField {
            stateMachine.markUnsupportedSecureField()
        }
        if let result = try? bridge?.read(BridgeResult.self, from: .result) {
            _ = stateMachine.apply(result: result)
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
        guard case .insertReady(let result) = stateMachine.state else { return }
        let inserter = TextInsertion(documentProxy: textDocumentProxy)
        guard inserter.insert(result) else {
            pasteboard.string = result.polished
            acknowledge(result: result, outcome: .copied)
            stateMachine.acknowledgeCopied(resultSeq: result.resultSeq)
            renderCurrentState()
            return
        }
        acknowledge(result: result, outcome: .inserted)
        stateMachine.acknowledgeInserted(resultSeq: result.resultSeq)
        renderCurrentState()
    }

    private func copyCurrentResult() {
        guard case .insertReady(let result) = stateMachine.state else { return }
        pasteboard.string = result.polished
        acknowledge(result: result, outcome: .copied)
        stateMachine.acknowledgeCopied(resultSeq: result.resultSeq)
        renderCurrentState()
    }

    private func saveCurrentResult() {
        guard case .insertReady(let result) = stateMachine.state else { return }
        acknowledge(result: result, outcome: .savedToHistory)
        stateMachine.acknowledgeSaved(resultSeq: result.resultSeq)
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
