import Foundation
import AirNoteShared

struct KeyboardCommandHandler {
    func makeStartSessionCommand(context: KeyboardContext) -> BridgeCommand {
        makeCommand(kind: .startSession, context: context)
    }

    func makeStartRecordingCommand(context: KeyboardContext) -> BridgeCommand {
        makeCommand(kind: .startRecording, context: context)
    }

    func makeStopRecordingCommand(context: KeyboardContext) -> BridgeCommand {
        makeCommand(kind: .stopRecording, context: context)
    }

    private func makeCommand(kind: BridgeCommandKind, context: KeyboardContext) -> BridgeCommand {
        BridgeCommand(
            kind: kind,
            commandSeq: UInt64(Date().timeIntervalSince1970 * 1000),
            keyboardContext: context,
            languageHint: .hinglish,
            style: .work,
            clientRequestID: RequestId.make()
        )
    }
}
