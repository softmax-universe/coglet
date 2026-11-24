import Foundation
import Speech
import AVFoundation
import SoftmaxCore

/// Explicit speech permission, on-device only. Never silently uploads PCM.
@MainActor final class LocalSpeechInput {
    private var recognition: SFSpeechRecognitionTask?
    private var pending: CheckedContinuation<String, any Error>?
    private var timeout: Task<Void, Never>?
    private enum Failure: LocalizedError {
        case unavailable
        var errorDescription: String? { "On-device speech recognition unavailable or permission denied. No audio was uploaded." }
    }
    func transcribe(_ utterance: Utterance) async throws -> String {
        let allowed = await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0 == .authorized) }
        }
        guard allowed, let recognizer = SFSpeechRecognizer(), recognizer.supportsOnDeviceRecognition else { throw Failure.unavailable }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false
        for frame in utterance.frames {
            guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: frame.sampleRate, channels: 1, interleaved: false),
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frame.samples.count)),
                  let channel = buffer.floatChannelData?[0] else { throw Failure.unavailable }
            buffer.frameLength = AVAudioFrameCount(frame.samples.count)
            frame.samples.withUnsafeBufferPointer { samples in
                if let base = samples.baseAddress { channel.update(from: base, count: samples.count) }
            }
            request.append(buffer)
        }
        request.endAudio()
        return try await withCheckedThrowingContinuation { continuation in
            pending = continuation
            recognition = recognizer.recognitionTask(with: request) { [weak self] result, error in
                let final = result?.isFinal == true
                let text = result?.bestTranscription.formattedString
                let failed = error != nil
                Task { @MainActor in
                    if final, let text { self?.finish(.success(text)) }
                    else if failed { self?.finish(.failure(Failure.unavailable)) }
                }
            }
            timeout = Task { [weak self] in
                try? await Task.sleep(for: .seconds(30))
                guard !Task.isCancelled else { return }
                self?.finish(.failure(Failure.unavailable))
            }
        }
    }
    private func finish(_ result: Result<String, any Error>) {
        guard let pending else { return }; self.pending = nil
        timeout?.cancel(); timeout = nil; recognition?.cancel(); recognition = nil
        pending.resume(with: result)
    }
}
