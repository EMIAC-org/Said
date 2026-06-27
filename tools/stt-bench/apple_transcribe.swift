import Foundation
import AVFoundation
import Speech

enum AppleTranscribeError: Error, CustomStringConvertible {
    case missingArgument
    case localeUnsupported(String)

    var description: String {
        switch self {
        case .missingArgument:
            return "usage: apple_transcribe.swift <audio-file> [locale]"
        case .localeUnsupported(let locale):
            return "Apple SpeechTranscriber does not support locale \(locale)"
        }
    }
}

func localeID(_ locale: Locale) -> String {
    locale.identifier(.bcp47)
}

func ensureModel(transcriber: SpeechTranscriber, locale: Locale) async throws {
    let wanted = localeID(locale)
    let supported = await SpeechTranscriber.supportedLocales
        .map { localeID($0) }
    guard supported.contains(wanted) else {
        throw AppleTranscribeError.localeUnsupported(wanted)
    }

    let installed = await Set(SpeechTranscriber.installedLocales.map { localeID($0) })
    if installed.contains(wanted) {
        return
    }

    if let downloader = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
        try await downloader.downloadAndInstall()
    }
}

@main
struct AppleTranscribe {
    static func main() async {
        do {
            guard CommandLine.arguments.count >= 2 else {
                throw AppleTranscribeError.missingArgument
            }

            let audioURL = URL(fileURLWithPath: CommandLine.arguments[1])
            let audioFile = try AVAudioFile(forReading: audioURL)
            let locale = Locale(identifier: CommandLine.arguments.count >= 3 ? CommandLine.arguments[2] : Locale.current.identifier)
            let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)

            try await ensureModel(transcriber: transcriber, locale: locale)

            async let transcriptionFuture: String = transcriber.results.reduce("") { (partial: String, result) in
                partial + String(result.text.characters)
            }

            let analyzer = SpeechAnalyzer(modules: [transcriber])
            if let lastSample = try await analyzer.analyzeSequence(from: audioFile) {
                try await analyzer.finalizeAndFinish(through: lastSample)
            } else {
                await analyzer.cancelAndFinishNow()
            }

            let transcription = try await transcriptionFuture
            print(transcription.trimmingCharacters(in: CharacterSet.whitespacesAndNewlines))
        } catch {
            fputs("apple_speech error: \(error)\n", stderr)
            exit(1)
        }
    }
}
