import Testing
import AVFoundation
import CAudioRing
import Dispatch
@testable import FaceAudioKit

@Test func ringWrapOverflowAndDiscard() throws {
    let ring = try PCMQueue(capacity: 4)
    #expect([Int16(1), 2, 3].withUnsafeBufferPointer { ring.write($0) })
    #expect(![Int16(4), 5].withUnsafeBufferPointer { ring.write($0) })
    var out = [Int16](repeating: 0, count: 2)
    #expect(out.withUnsafeMutableBufferPointer { fa_read(ring.raw, $0.baseAddress, 2) } == 2)
    #expect(out == [1, 2])
    #expect([Int16(4), 5, 6].withUnsafeBufferPointer { ring.write($0) })
    var all = [Int16](repeating: 0, count: 4)
    #expect(all.withUnsafeMutableBufferPointer { fa_read(ring.raw, $0.baseAddress, 4) } == 4)
    #expect(all == [3, 4, 5, 6])
    #expect([Int16(7), 8].withUnsafeBufferPointer { ring.write($0) })
    ring.flush()
    #expect(all.withUnsafeMutableBufferPointer { fa_read(ring.raw, $0.baseAddress, 4) } == 0)
    #expect([Int16(9)].withUnsafeBufferPointer { ring.write($0) })
    #expect(all.withUnsafeMutableBufferPointer { fa_read(ring.raw, $0.baseAddress, 4) } == 1)
    #expect(all[0] == 9)
}

@Test func concurrentSPSC() throws {
    let ring = try PCMQueue(capacity: 128)
    let group = DispatchGroup()
    group.enter()
    DispatchQueue.global().async {
        for i in 0..<100000 {
            var sample = Int16(i % 30000)
            while !withUnsafePointer(to: &sample, { fa_write(ring.raw, $0, 1) }) { }
        }
        group.leave()
    }
    for i in 0..<100000 {
        var sample: Int16 = -1
        while withUnsafeMutablePointer(to: &sample, { fa_read(ring.raw, $0, 1) }) == 0 { }
        #expect(sample == Int16(i % 30000))
    }
    group.wait()
}

@Test func playbackGenerationFlushAndPosition() throws {
    let ring = try PCMQueue(capacity: 8)
    #expect([Int16(16384), -16384].withUnsafeBufferPointer { ring.write($0) })
    var out = [Float](repeating: 99, count: 4)
    #expect(out.withUnsafeMutableBufferPointer { fa_render(ring.raw, $0.baseAddress, 4, 1) } == 2)
    #expect(out == [0.5, -0.5, 0, 0])
    #expect(fa_played(ring.raw) == 2)
    let g = fa_generation(ring.raw)
    #expect([Int16(1000), 1000].withUnsafeBufferPointer { ring.write($0) })
    ring.flush()
    #expect(fa_generation(ring.raw) != g)
    #expect(out.withUnsafeMutableBufferPointer { fa_render(ring.raw, $0.baseAddress, 4, 1) } == 0)
    #expect(out == [0, 0, 0, 0])
    #expect(fa_played(ring.raw) == 2)
    #expect([Int16(16384)].withUnsafeBufferPointer { ring.write($0) })
    fa_gain(ring.raw, 0.25)
    #expect(out.withUnsafeMutableBufferPointer { fa_render(ring.raw, $0.baseAddress, 4, 1) } == 1)
    #expect(out[0] == 0.125)
}

@Test func overlappingFlushPreservesNewUtterance() throws {
    let ring = try PCMQueue(capacity: 8)
    let generation = fa_generation(ring.raw)
    ring.flush()
    #expect([Int16(16384)].withUnsafeBufferPointer { ring.write($0) })
    var out = [Float](repeating: 99, count: 4)
    #expect(out.withUnsafeMutableBufferPointer {
        fa_render_generation(ring.raw, $0.baseAddress, 4, 1, generation)
    } == 0)
    #expect(out == [0, 0, 0, 0])
    #expect(out.withUnsafeMutableBufferPointer { fa_render(ring.raw, $0.baseAddress, 4, 1) } == 1)
    #expect(out[0] == 0.5)
}

@Test func conversion48kStereoTo24kMonoPCM16() throws {
    let format = try #require(AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 2))
    let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4800))
    buffer.frameLength = 4800
    for ch in 0..<2 { for i in 0..<4800 { buffer.floatChannelData![ch][i] = 0.25 } }
    let converter = try CaptureConverter(format: format)
    var samples: [Int16] = []
    for _ in 0..<4 { samples.append(contentsOf: try converter.convert(buffer)) }
    #expect(samples.count > 9000)
    #expect(samples.count <= 9600)
    #expect(samples.suffix(100).allSatisfy { abs(Int($0) - 8192) < 20 })
}

@Test func energyHangover() {
    var vad = EnergyVAD()
    let quiet = vad.process([Int16](repeating: 0, count: 2400))
    let voice = vad.process([Int16](repeating: 5000, count: 2400))
    let tail = vad.process([Int16](repeating: 0, count: 2400))
    let expired = vad.process([Int16](repeating: 0, count: 2400))
    #expect(!quiet && voice && tail && !expired)
}

@MainActor final class FakeEngine: DuplexEngine {
    var stopped = false
    var fail = false
    var changed: (@MainActor @Sendable () -> Void)?
    var configuration: AudioConfiguration?
    func start(configuration: AudioConfiguration, capture: PCMQueue, playback: PCMQueue,
               changed: @escaping @MainActor @Sendable () -> Void) throws {
        self.configuration = configuration; self.changed = changed
        if fail { throw FaceAudioError.aecUnavailable }
    }
    func stop() { stopped = true }
}

@Test @MainActor func configurationRebuildIgnoresStaleGraphAndFailsClosed() throws {
    var graphs: [FakeEngine] = []
    let kit = FaceAudioKit(factory: {
        let graph = FakeEngine(); graphs.append(graph); return graph
    }, permission: { true })
    try kit.startCapture()
    let firstGeneration = kit.graphGeneration
    try kit.enqueuePlayback([1, 2])
    graphs[0].changed?()
    #expect(graphs.count == 2)
    #expect(graphs[0].stopped)
    #expect(kit.graphGeneration > firstGeneration)
    graphs[0].changed?()
    #expect(graphs.count == 2)
    try kit.selectDevices(.init(inputUID: "mic", outputUID: "speaker", echoCancellation: false))
    #expect(graphs.count == 3)
    #expect(graphs[2].configuration == .init(inputUID: "mic", outputUID: "speaker", echoCancellation: false))
    kit.stopCapture()
    graphs[2].changed?()
    #expect(!kit.isRunning)
    #expect(graphs.count == 3)
    let failed = FakeEngine(); failed.fail = true
    let bad = FaceAudioKit(factory: { failed }, permission: { true })
    #expect(throws: (any Error).self) { try bad.startCapture() }
    #expect(!bad.isRunning)
    #expect(failed.stopped)
    var rebuilds = 0
    let initial = FakeEngine()
    let recovery = FaceAudioKit(factory: {
        rebuilds += 1
        return rebuilds == 1 ? initial : failed
    }, permission: { true })
    try recovery.startCapture()
    initial.changed?()
    #expect(!recovery.isRunning)
    #expect(recovery.lastError != nil)
    initial.changed?()
    #expect(rebuilds == 2)
    let denied = FaceAudioKit(factory: { FakeEngine() }, permission: { false })
    #expect(throws: (any Error).self) { try denied.startCapture() }
}
