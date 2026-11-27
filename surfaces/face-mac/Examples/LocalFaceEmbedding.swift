#if os(macOS)
import Foundation
import FaceMac
import FaceAudioKit
import CogletSurface

/// Compile this in the embedding app, not in a second command-line audio host.
/// The app supplies its renderer, qualified runtime link, grants and durable store.
/// Its app manifest needs NSMicrophoneUsageDescription and
/// NSSpeechRecognitionUsageDescription (plus audio-input entitlement if sandboxed).
/// Request OS permissions only from explicit app UI before calling activateDuplex;
/// these adapters intentionally never request permissions or download voices.
@MainActor struct LocalFaceEmbedding {
    let kit: FaceAudioKit
    let recognition: AppleLocalRecognition
    let backend: FaceSurfaceBackend

    init(installedVoiceIdentifier: String, renderer: FaceExpressionRenderer,
         capturePositionMS: @escaping @MainActor () -> UInt64) throws {
        let kit = FaceAudioKit()
        self.kit = kit
        recognition = AppleLocalRecognition(locale: Locale(identifier: "en-US"), positionMS: capturePositionMS)
        let synthesis = try AppleLocalSynthesis(voiceIdentifier: installedVoiceIdentifier)
        backend = FaceSurfaceBackend(audio: NativeFaceDuplex(kit: kit), synthesis: synthesis,
                                     renderer: renderer, sttRoute: "local", ttsRoute: "local")
    }

    /// Call only after host.start and its normal authenticated configuration/grant
    /// exchange. The same qualified guest/WASI runtime still executes the coglet;
    /// this example does not substitute an in-process message or action shortcut.
    func enableCapture(on host: FaceSurfaceHost, explicitLocalConsent: Bool) throws {
        try host.activateDuplex(localConsent: explicitLocalConsent, remoteAudioConsent: false)
        try host.enableVoiceDetection(recognition: recognition)
    }

    // Route synthesis returns canonical24kMonoInt16 PCM to the existing backend.
    // Only NativeFaceDuplex plays it. Its actual render counter drives position,
    // message outcomes and heard history. Unknown word alignment stays empty.
    // The configured local adapter/router is not a new wire protocol. Back channel
    // acknowledgements use the existing speech.backchannel admission path.
}
#endif
