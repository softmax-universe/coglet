import Testing
import AVFoundation
import CAudioRing
@testable import FaceAudioKit

// VoiceProcessingIO on a mono USB microphone plus stereo output can expose
// a discrete three-channel tap. AVAudioConverter's implicit mono downmix
// silently produces zeros for this layout, despite nonzero native input.
private func multichannelCaptureBuffer(microphone: Float, auxiliary: Float) throws -> AVAudioPCMBuffer {
    let layout = try #require(AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | 3))
    let format = AVAudioFormat(standardFormatWithSampleRate: 48000, channelLayout: layout)
    let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4800))
    buffer.frameLength = 4800
    for channel in 0..<3 {
        for frame in 0..<4800 {
            buffer.floatChannelData![channel][frame] = channel == 0 ? microphone : auxiliary
        }
    }
    return buffer
}

@Test func conversionDiscreteThreeChannelCapturePreservesMicrophone() throws {
    let buffer = try multichannelCaptureBuffer(microphone: 0.25, auxiliary: -0.5)
    let converter = try CaptureConverter(format: buffer.format)
    var samples: [Int16] = []
    for _ in 0..<4 { samples.append(contentsOf: try converter.convert(buffer)) }
    #expect(samples.count > 9000 && samples.count <= 9600)
    #expect(samples.suffix(100).allSatisfy { abs(Int($0) - 8192) < 20 })
}

@Test func conversionDiscreteCaptureDoesNotMixAuxiliaryChannelsIntoMicrophone() throws {
    let buffer = try multichannelCaptureBuffer(microphone: 0, auxiliary: 0.5)
    let converter = try CaptureConverter(format: buffer.format)
    var samples: [Int16] = []
    for _ in 0..<4 { samples.append(contentsOf: try converter.convert(buffer)) }
    #expect(samples.count > 9000 && samples.count <= 9600)
    #expect(samples.allSatisfy { $0 == 0 })
}

@Test @MainActor func productionMultichannelCaptureCallbackPreservesMicrophoneOffMainActor() async throws {
    let capture = try PCMQueue(capacity: 24000)
    let buffer = try multichannelCaptureBuffer(microphone: 0.25, auxiliary: -0.5)
    let tap = try AudioCallbacks.tap(capture: capture, format: buffer.format)
    try await Task.detached {
        dispatchPrecondition(condition: .notOnQueue(.main))
        let sdkTap: AVAudioNodeTapBlock = tap
        // Allocate on the callback thread; no AVFoundation object crosses actors.
        let input = try multichannelCaptureBuffer(microphone: 0.25, auxiliary: -0.5)
        for _ in 0..<4 { sdkTap(input, AVAudioTime(sampleTime: 0, atRate: 48000)) }
        #expect(!fa_failed(capture.raw))
        var samples = [Int16](repeating: 0, count: 24000)
        let count = samples.withUnsafeMutableBufferPointer { fa_read(capture.raw, $0.baseAddress, $0.count) }
        try #require(count > 9000 && count <= 9600)
        #expect(samples[(count - 100)..<count].allSatisfy { abs(Int($0) - 8192) < 20 })
        #expect(fa_peak(capture.raw) > 0.24)
    }.value
}
