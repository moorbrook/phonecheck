// speech_helper — on-device transcription via Apple's SpeechAnalyzer/SpeechTranscriber
// (macOS 26 Speech framework).
//
// Usage:   speech_helper <wav-path>
// Output:  JSON on stdout: {"transcript": "..."}
// Errors:  message on stderr, non-zero exit code.
//
// The en-US model assets are system assets managed by the OS (AssetInventory).
// If they are not yet installed, this helper requests a one-time OS-managed
// download — the app itself never bundles or fetches a model.
//
// Built by scripts/build_speech_helper.sh (invoked automatically from build.rs).
import Foundation
import Speech
import AVFoundation

func fail(_ message: String, code: Int32 = 1) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

@main
struct SpeechHelper {
    static func main() async {
        guard #available(macOS 26.0, *) else {
            fail("SpeechAnalyzer requires macOS 26.0 or later")
        }
        guard CommandLine.arguments.count == 2 else {
            fail("usage: speech_helper <wav-path>", code: 2)
        }
        do {
            try await run(wavPath: CommandLine.arguments[1])
        } catch {
            fail("transcription failed: \(error)")
        }
    }

    @available(macOS 26.0, *)
    static func run(wavPath: String) async throws {
        let locale = Locale(identifier: "en_US")

        let supported = await SpeechTranscriber.supportedLocales
        guard supported.contains(where: { $0.identifier(.bcp47) == "en-US" }) else {
            fail("SpeechTranscriber does not support en-US on this OS")
        }

        let transcriber = SpeechTranscriber(
            locale: locale,
            transcriptionOptions: [],
            reportingOptions: [],
            attributeOptions: []
        )

        // Ensure the OS-managed en-US model assets are installed (one-time).
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            FileHandle.standardError.write(Data("installing en-US speech assets (OS-managed, one-time)...\n".utf8))
            try await request.downloadAndInstall()
        }
        let status = await AssetInventory.status(forModules: [transcriber])
        guard status == .installed else {
            fail("en-US speech assets not installed (status: \(status)); check network and retry")
        }

        let file = try AVAudioFile(forReading: URL(fileURLWithPath: wavPath))
        let analyzer = SpeechAnalyzer(modules: [transcriber])

        let collector = Task {
            var text = ""
            for try await result in transcriber.results where result.isFinal {
                text += String(result.text.characters)
            }
            return text
        }

        if let lastSample = try await analyzer.analyzeSequence(from: file) {
            try await analyzer.finalizeAndFinish(through: lastSample)
        } else {
            await analyzer.cancelAndFinishNow()
        }

        let transcript = try await collector.value
            .trimmingCharacters(in: .whitespacesAndNewlines)

        let payload = try JSONSerialization.data(withJSONObject: ["transcript": transcript])
        FileHandle.standardOutput.write(payload)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
