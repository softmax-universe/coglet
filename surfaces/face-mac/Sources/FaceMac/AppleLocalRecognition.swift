#if os(macOS)
import Foundation
@preconcurrency import AVFoundation
@preconcurrency import Speech

/// Requires permissions granted by the embedding app *before* starting. This
/// library never prompts, opens a microphone, or permits server recognition.
@MainActor public final class AppleLocalRecognition: FaceRecognition {
    public let route = "local"
    public let usesRemoteAudio = false
    private let locale: Locale
    private let positionMS: @MainActor () -> UInt64
    private weak var current: AppleRecognitionInput?

    /// Supply the host's canonical capture timeline, not elapsed wall time.
    public init(locale: Locale = .current, positionMS: @escaping @MainActor () -> UInt64) {
        self.locale = locale; self.positionMS = positionMS
    }
    public func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
                      failed: @escaping @MainActor () throws -> Void) throws -> any FaceRecognitionInput {
        current?.cancel()
        guard SFSpeechRecognizer.authorizationStatus() == .authorized,
              AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw LocalSpeechError.permissionDenied
        }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable,
              recognizer.supportsOnDeviceRecognition else { throw LocalSpeechError.unavailable }
        let input = AppleRecognitionInput(recognizer: recognizer, positionMS: positionMS,
                                          receive: receive, failed: failed)
        current = input
        input.start()
        return input
    }
}

struct AppleRecognitionSnapshot: Sendable {
    let text: String
    let final: Bool
    let failed: Bool
}

/// One coalescing mailbox bounds queued private text even if MainActor is busy.
/// SDK objects/errors never cross the actor hop, and no private text is logged.
private final class AppleRecognitionMailbox: @unchecked Sendable {
    private let lock = NSLock()
    private var latest: AppleRecognitionSnapshot?
    private var scheduled = false
    private var active = true
    func cancel() { lock.lock(); defer { lock.unlock() }; active = false; latest = nil }
    func take() -> AppleRecognitionSnapshot? {
        lock.lock(); defer { lock.unlock() }
        scheduled = false
        let value = latest; latest = nil
        return active ? value : nil
    }
    private func put(_ value: AppleRecognitionSnapshot) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard active, latest?.final != true, latest?.failed != true else { return false }
        latest = value
        if scheduled { return false }; scheduled = true; return true
    }
    nonisolated static func callback(_ mailbox: AppleRecognitionMailbox,
        deliver: @escaping @MainActor @Sendable () -> Void
    ) -> @Sendable (SFSpeechRecognitionResult?, Error?) -> Void {
        { @Sendable result, error in
            let text = result?.bestTranscription.formattedString ?? ""
            let invalid = text.utf8.count > 16_384
            let value = AppleRecognitionSnapshot(text: invalid ? "" : text,
                final: result?.isFinal ?? false, failed: invalid || result == nil || (error != nil && result?.isFinal != true))
            if mailbox.put(value) { Task { @MainActor in deliver() } }
        }
    }
}

@MainActor final class AppleRecognitionInput: FaceRecognitionInput {
    private let recognizer: SFSpeechRecognizer
    private let request = SFSpeechAudioBufferRecognitionRequest()
    private var task: SFSpeechRecognitionTask?
    private let mailbox = AppleRecognitionMailbox()
    private let positionMS: @MainActor () -> UInt64
    private let receive: @MainActor (FaceRecognitionUpdate) throws -> Void
    private let failed: @MainActor () throws -> Void
    private var active = true
    private var finishing = false
    private var frames: UInt64 = 0
    private var revision: UInt64 = 0
    init(recognizer: SFSpeechRecognizer, positionMS: @escaping @MainActor () -> UInt64,
         receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
         failed: @escaping @MainActor () throws -> Void) {
        self.recognizer = recognizer; self.positionMS = positionMS
        self.receive = receive; self.failed = failed
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
    }
    func start() {
        task = recognizer.recognitionTask(with: request, resultHandler: AppleRecognitionMailbox.callback(mailbox) { [weak self] in
            self?.drain()
        })
    }
    private func drain() {
        guard active, let value = mailbox.take() else { return }
        deliver(value)
    }
    /// Snapshot seam shares the native callback's actual retirement/delivery path.
    func deliver(_ value: AppleRecognitionSnapshot) {
        guard active else { return }
        if value.failed { fail(); return }
        revision += 1
        let update = FaceRecognitionUpdate(revision: revision, text: value.text, final: value.final,
                                           positionMS: positionMS())
        if value.final { cancel() }
        do { try receive(update) }
        catch {
            // A final retires native input before delivery. Its throwing receiver
            // still needs one failure notification; fail() would ignore inactive.
            if value.final { try? failed() } else { fail() }
        }
    }
    func append(_ samples: [Int16]) throws {
        guard active, !finishing else { throw LocalSpeechError.cancelled }
        guard samples.count <= 96_000, frames + UInt64(samples.count) <= 24_000 * 60 else {
            fail(); throw LocalSpeechError.resourceExhausted
        }
        guard SFSpeechRecognizer.authorizationStatus() == .authorized,
              AVCaptureDevice.authorizationStatus(for: .audio) == .authorized,
              recognizer.isAvailable, recognizer.supportsOnDeviceRecognition else {
            fail(); throw LocalSpeechError.unavailable
        }
        guard !samples.isEmpty else { return }
        guard let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 24_000,
                                          channels: 1, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
              let data = buffer.int16ChannelData else { fail(); throw LocalSpeechError.invalidFormat }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { source in data[0].update(from: source.baseAddress!, count: source.count) }
        frames += UInt64(samples.count)
        request.append(buffer)
    }
    func finish() {
        guard active, !finishing else { return }
        finishing = true; request.endAudio()
    }
    func cancel() {
        active = false; mailbox.cancel(); task?.cancel(); task = nil; request.endAudio()
    }
    private func fail() {
        guard active else { return }
        cancel(); try? failed()
    }
}
#endif
