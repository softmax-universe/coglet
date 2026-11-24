import XCTest
import CogletTypes
import CogletSurface
@testable import FaceMac

@MainActor final class TestDuplex: FaceDuplexIO {
    var renderedFrames: UInt64 = 0
    var isRunning = false
    var generation: UInt64 = 0
    var captureFailed = false
    var enqueued: [[Int16]] = []
    var captured: [Int16] = []
    var flushes = 0
    func start() throws { isRunning = true }
    func stop() { isRunning = false; generation += 1 }
    func enqueue(_ samples: [Int16]) throws { enqueued.append(samples) }
    func flush() { flushes += 1 }
    func readCapture() -> [Int16] { let value = captured; captured = []; return value }
}
@MainActor final class TestSynthesis: FaceSynthesis {
    var callbacks: [(Result<FaceSynthesizedAudio, Swift.Error>) -> Void] = []
    var cancelled = 0
    var immediateFailure = false
    func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, Swift.Error>) -> Void) -> @MainActor () -> Void {
        callbacks.append(completion)
        if immediateFailure { completion(.failure(NativeSurfaceError("unavailable"))) }
        return { self.cancelled += 1 }
    }
}

final class FaceSurfaceTests: XCTestCase {
    func testBargeInFlushesBeforeEventsCancelsQueueAndFencesLateSynthesis() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            var events: [String] = []
            face.activate(owner: Self.owner, epoch: 1) { kind, _ in
                if kind == "speech.barge_in" { XCTAssertGreaterThan(io.flushes, 1) }
                events.append(kind)
            }
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000001", final: false), epoch: 1, now: 0)
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000002", chunk: 1), epoch: 1, now: 1)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 24_000))))
            try face.tick(now: 2); events = []
            io.renderedFrames = 12_000
            try face.speechStart(utterance: "20000000000000000000000000000000", positionMS: 100)
            XCTAssertEqual(events, ["speech.start", "speech.barge_in", "speech.position"])
            let done = face.takeCompletions()
            XCTAssertEqual(done.count, 2)
            for result in done { if case .cancelled = result.result {} else { XCTFail("must cancel, not fail") } }
            let count = io.enqueued.count
            tts.callbacks[0](.success(.init(samples: [1, 2, 3])))
            try face.tick(now: 3)
            XCTAssertEqual(io.enqueued.count, count)
            XCTAssertThrowsError(try face.preflight(action: Self.speak(id: "00000000000000000000000000000003", chunk: 2), now: 4))
            let heard = try face.read(operation: "speech.heard", args: .record([("item", .text("00000000000000000000000000000001"))]), owner: Self.owner, now: 4)
            XCTAssertEqual(heard["played_ms"], .unsigned(500))
            XCTAssertEqual(heard["text"], .text(""))
        }
    }
    func testDeadlineStopsQueuedAudioAndFreezesHeardBeforeLateCallback() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000001", final: false), epoch: 1, now: 0)
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000002", chunk: 1), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 24_000), spans: [.init(endMS: 400, text: "hello ")])))
            try face.tick(now: 1)
            io.renderedFrames = 12_000
            try face.tick(now: 2_000_000)
            let record = try face.read(operation: "speech.heard", args: .record([("item", .text("00000000000000000000000000000001"))]), owner: Self.owner, now: 2_000_000)
            XCTAssertEqual(record["played_ms"], .unsigned(500))
            XCTAssertEqual(record["text"], .text("hello "))
            let results = face.takeCompletions()
            XCTAssertEqual(results.count, 2)
            if case .cancelled = results.last?.result {} else { XCTFail("later chunks must cancel") }
            tts.callbacks[0](.success(.init(samples: [1])))
            try face.tick(now: 2_000_001)
            XCTAssertEqual(io.enqueued.count, 1)
        }
    }
    func testIdleBackchannelIsRenderedAndItsItemIsWireActionID() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            var positions: [CBORValue] = []
            face.activate(owner: Self.owner, epoch: 1) { _, payload in positions.append(payload) }
            let action = CBORValue.record([("id", .text("00000000000000000000000000000009")), ("coglet", .text(Self.owner)),
                ("capability", .text("speech.backchannel")), ("deadline", .unsigned(2_000_000)),
                ("args", .record([("stream", .text("30000000000000000000000000000000")), ("cue", .text("ack"))]))])
            _ = try face.begin(action: action, epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400))))
            try face.tick(now: 1)
            XCTAssertTrue(face.takeCompletions().isEmpty)
            io.renderedFrames = 2400; try face.tick(now: 2)
            XCTAssertEqual(face.takeCompletions().first?.id, "00000000000000000000000000000009")
            XCTAssertEqual(positions.last?["item"], action["id"])
            XCTAssertThrowsError(try face.preflight(action: action, now: 3))
        }
    }
    func testStopIsIdempotentAndRetainsFinalHeard() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            let a = Self.speak(id: "00000000000000000000000000000001")
            _ = try face.begin(action: a, epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 24_000))))
            try face.tick(now: 1); io.renderedFrames = 12_000
            let stop = CBORValue.record([("id", .text("00000000000000000000000000000002")), ("coglet", .text(Self.owner)),
                ("capability", .text("speech.stop")), ("deadline", .unsigned(2_000_000)),
                ("args", .record([("stream", .text("10000000000000000000000000000000"))]))])
            if case .succeeded = try face.begin(action: stop, epoch: 1, now: 2) {} else { XCTFail("stop did not succeed") }
            if case .cancelled = face.takeCompletions().first?.result {} else { XCTFail("chunk not cancelled") }
            _ = try face.begin(action: stop, epoch: 1, now: 3)
            XCTAssertTrue(face.takeCompletions().isEmpty)
            XCTAssertEqual(try face.read(operation: "speech.heard", args: .record([("item", a["id"]!)]), owner: Self.owner, now: 4)["played_ms"], .unsigned(500))
        }
    }
    func testSpeechConfigReadIsConfiguredSnapshotWithoutFallback() async throws {
        try await MainActor.run {
            let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            let config = try face.read(operation: "speech.config", args: .map([]), owner: Self.owner, now: 0)
            try CBORCodec.validate("speech::speech-config", config)
            XCTAssertEqual(config["barge"], .text("stop"))
            XCTAssertEqual(config["session_mode"], .text("text"))
            XCTAssertThrowsError(try face.read(operation: "speech.config", args: .map([]), owner: "other", now: 0))
        }
    }
    func testExpressionWireActionAndStateReadUseNativeExpiry() async throws {
        try await MainActor.run {
            let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            let action = CBORValue.record([("id", .text("00000000000000000000000000000001")), ("coglet", .text(Self.owner)),
                ("capability", .text("expression.set_expression")), ("deadline", .unsigned(2_000_000)),
                ("args", .record([("expression", .text("happy")), ("intensity", .unsigned(700)), ("duration_ms", .unsigned(100))]))])
            try face.preflight(action: action, now: 0)
            XCTAssertEqual(face.renderer.snapshot(now: 0).expression, "neutral", "preflight has no native effects")
            if case .succeeded = try face.begin(action: action, epoch: 1, now: 0) {} else { XCTFail("renderer must apply synchronously") }
            let value = try face.read(operation: "expression.state", args: .map([]), owner: Self.owner, now: 50_000)
            XCTAssertEqual(value["expression"], .text("happy")); XCTAssertEqual(value["remaining_ms"], .unsigned(50))
            try face.tick(now: 100_000)
            XCTAssertEqual(try face.read(operation: "expression.state", args: .map([]), owner: Self.owner, now: 100_000)["expression"], .text("neutral"))
        }
    }
    func testDuplicateSynthesisCompletionCannotReplacePlayingMedia() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000001"), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: [Int16](repeating: 1, count: 2400))))
            try face.tick(now: 1)
            tts.callbacks[0](.success(.init(samples: [Int16](repeating: 1, count: 24))))
            tts.callbacks[0](.failure(NativeSurfaceError("unavailable")))
            io.renderedFrames = 24; try face.tick(now: 2)
            XCTAssertTrue(face.takeCompletions().isEmpty)
            io.renderedFrames = 2400; try face.tick(now: 3)
            if case .succeeded = face.takeCompletions().first?.result {} else { XCTFail("first synthesis result must win") }
        }
    }
    func testSynchronousSynthesisFailureCancelsReturnedHandleBeforeNewWork() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis(); tts.immediateFailure = true
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000001"), epoch: 1, now: 0)
            XCTAssertEqual(tts.cancelled, 1)
            XCTAssertEqual(face.takeCompletions().count, 1)
            face.stop(owner: nil)
            XCTAssertEqual(tts.cancelled, 1, "failed handle must not remain installed")
        }
    }
    func testUnfinalizedStreamClosesAtSixtySecondsWithoutAnotherChunk() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1, emit: { _, _ in })
            _ = try face.begin(action: Self.speak(id: "00000000000000000000000000000001", final: false), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: [Int16](repeating: 1, count: 24))))
            try face.tick(now: 1); io.renderedFrames = 24; try face.tick(now: 2)
            _ = face.takeCompletions()
            try face.tick(now: 59_999_999)
            try face.preflight(action: Self.speak(id: "00000000000000000000000000000002", chunk: 1), now: 59_999_999)
            try face.tick(now: 60_000_000)
            XCTAssertThrowsError(try face.preflight(action: Self.speak(id: "00000000000000000000000000000002", chunk: 1), now: 60_000_000))
        }
    }
    private static let owner = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    func testSuccessRequiresRenderedAudioAndHeardUsesOnlyAlignedPrefix() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            face.activate(owner: "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", epoch: 1, emit: { _, _ in })
            let action = Self.speak(id: "00000000000000000000000000000001")
            try face.preflight(action: action, now: 0)
            XCTAssertNil(try face.begin(action: action, epoch: 1, now: 0))
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 24_000), spans: [.init(endMS: 400, text: "hello "), .init(endMS: 1000, text: "world")])))
            try face.tick(now: 10)
            XCTAssertTrue(face.takeCompletions().isEmpty)
            io.renderedFrames = 12_000
            try face.tick(now: 20)
            XCTAssertEqual(try face.read(operation: "speech.heard", args: .record([("item", action["id"]!)]), owner: "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", now: 20)["text"], .text("hello "))
            XCTAssertTrue(face.takeCompletions().isEmpty)
            io.renderedFrames = 24_000
            try face.tick(now: 30)
            let results = face.takeCompletions()
            XCTAssertEqual(results.count, 1)
            if case .succeeded = results.first?.result {} else { XCTFail("rendered item did not succeed") }
        }
    }
    private static func speak(id: String, chunk: UInt64 = 0, final: Bool = true) -> CBORValue {
        .record([("id", .text(id)), ("coglet", .text("coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")), ("capability", .text("speech.speak_chunk")), ("deadline", .unsigned(2_000_000)), ("args", .record([("stream", .text("10000000000000000000000000000000")), ("chunk", .unsigned(chunk)), ("text", .text("hello world")), ("final", .boolean(final))]))])
    }
    func testExpressionReplacementAndNativeExpiry() async throws {
        try await MainActor.run {
            var painted: [FaceExpression] = []
            let renderer = FaceExpressionRenderer { painted.append($0) }
            try renderer.set(expression: "thinking", intensity: 800, durationMS: 100, now: 0)
            try renderer.set(expression: "happy", intensity: 500, durationMS: 200, now: 50_000)
            renderer.tick(now: 100_000)
            XCTAssertEqual(renderer.snapshot(now: 100_000).expression, "happy")
            renderer.tick(now: 250_000)
            XCTAssertEqual(renderer.snapshot(now: 250_000).expression, "neutral")
            XCTAssertEqual(painted.map(\.expression), ["thinking", "happy", "neutral"])
        }
    }
}
