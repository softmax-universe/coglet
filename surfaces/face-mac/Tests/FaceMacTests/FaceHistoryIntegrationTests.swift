import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class IntegrationLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var fails = false
    var onSave: (() -> Void)?
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws {
        onSave?()
        if fails { throw SurfaceFailure.closed }
        self.bytes = bytes
    }
}

final class FaceHistoryIntegrationTests: XCTestCase {
    private static let owner = "coglet:" + String(repeating: "a", count: 64)
    private static let identity = "surface:" + String(repeating: "b", count: 64)
    private static let item = String(repeating: "1", count: 32)
    private static let stream = String(repeating: "2", count: 32)

    func testHeardRejectsAdditionalFieldsRatherThanReturningHistory() async throws {
        try await MainActor.run {
            let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
            face.activate(owner: Self.owner, epoch: 1) { _, _ in }
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            XCTAssertThrowsError(try face.read(operation: "speech.heard", args: .record([
                ("item", .text(Self.item)), ("extra", .boolean(true))]), owner: Self.owner, now: 0))
        }
    }

    func testHostAdvertisesHistoryOnlyWithSeparateStoreAndPurgesOldEpoch() async throws {
        try await MainActor.run {
            for enabled in [false, true] {
                let store = IntegrationLedger()
                let history = try FaceHeardHistory(store: store)
                try history.register(item: Self.item, owner: Self.owner, epoch: 1)
                let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: IntegrationLedger(),
                    backend: FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in }),
                    grants: [], clock: { 0 }, historyStore: enabled ? store : nil)
                try host.start()
                let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
                try host.receive(SurfaceWire.encode(.message("hello_reply", .record([
                    ("selection", .record([("version", .text("0.1"))])),
                    ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
                let capabilities = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
                XCTAssertEqual(String(describing: capabilities).contains("speech.history"), enabled)
                XCTAssertFalse(FaceSurfaceHost.operations.contains { $0.entry == "speech.history" })
                if enabled {
                    let restored = try FaceHeardHistory(store: store)
                    XCTAssertThrowsError(try restored.heard(item: Self.item, owner: Self.owner, epoch: 1))
                }
                try host.close()
            }
        }
    }

    func testAppendSaveFailureThrowsBeforeEnqueueAndStaysLatchedAcrossActivation() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            try face.configureHistory(store: store)
            face.activate(owner: Self.owner, epoch: 1) { _, _ in XCTFail("no failed history observation") }
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400), spans: [.init(endMS: 100, text: "hello")])))
            store.fails = true
            XCTAssertThrowsError(try face.tick(now: 1))
            XCTAssertTrue(io.enqueued.isEmpty)
            store.fails = false
            face.activate(owner: Self.owner, epoch: 2) { _, _ in }
            XCTAssertThrowsError(try face.startDuplex())
            XCTAssertThrowsError(try face.configureHistory(store: IntegrationLedger()))
        }
    }

    func testSpeechStopSaveFailureCannotReturnSuccessOrObservation() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            try face.configureHistory(store: store)
            var events = 0
            face.activate(owner: Self.owner, epoch: 1) { _, _ in events += 1 }
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400))))
            try face.tick(now: 1); events = 0; store.fails = true
            XCTAssertThrowsError(try face.begin(action: Self.stopAction(), epoch: 1, now: 2))
            XCTAssertEqual(events, 0)
            XCTAssertThrowsError(try face.begin(action: Self.stopAction(), epoch: 1, now: 3))
            XCTAssertThrowsError(try face.tick(now: 3))
            XCTAssertThrowsError(try face.read(operation: "speech.heard", args: .record([("item", .text(Self.item))]), owner: Self.owner, now: 2))
        }
    }

    func testHistoryIsPersistedBeforeSynthesisAndPositionEmitter() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            try face.configureHistory(store: store)
            var emitted = 0
            face.activate(owner: Self.owner, epoch: 1) { kind, payload in
                guard kind == "speech.position" else { return }
                let restored = try FaceHeardHistory(store: store)
                let heard = try restored.heard(item: Self.item, owner: Self.owner, epoch: 1)
                XCTAssertEqual(heard["played_ms"], payload["played_ms"])
                XCTAssertEqual(heard["text"], payload["played_ms"] == .unsigned(100) ? .text("hello") : .text(""))
                emitted += 1
            }
            store.onSave = { XCTAssertTrue(tts.callbacks.isEmpty, "registration precedes native synthesis") }
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            store.onSave = nil
            XCTAssertEqual(try FaceHeardHistory(store: store).heard(item: Self.item, owner: Self.owner, epoch: 1)["text"], .text(""))
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400), spans: [.init(endMS: 100, text: "hello")])))
            try face.tick(now: 1); io.renderedFrames = 2400; try face.tick(now: 2)
            XCTAssertEqual(emitted, 2)
        }
    }

    func testRevokeStopsNativeAndRendererBeforeEverySaveAndDeletesPrivateRecords() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex(), tts = TestSynthesis()
            let renderer = FaceExpressionRenderer { _ in }
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: renderer)
            try face.configureHistory(store: store)
            face.activate(owner: Self.owner, epoch: 1) { _, _ in }
            try face.startDuplex()
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400), spans: [.init(endMS: 100, text: "hello")])))
            try face.tick(now: 1)
            try renderer.set(expression: "happy", intensity: 50, durationMS: 1000, now: 1)
            var saves = 0
            store.onSave = {
                XCTAssertFalse(io.isRunning)
                XCTAssertEqual(renderer.snapshot(now: 1).expression, "neutral")
                saves += 1
            }
            face.stop(owner: Self.owner)
            XCTAssertGreaterThan(saves, 0)
            let restored = try FaceHeardHistory(store: store)
            XCTAssertThrowsError(try restored.heard(item: Self.item, owner: Self.owner, epoch: 1))
            XCTAssertEqual(try CBORCodec.decodeProfile(store.bytes)["items"], .map([]))
        }
    }

    func testRevokeSaveFailurePreventsReactivationEvenAfterStoreRecovers() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex()
            let face = FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in })
            try face.configureHistory(store: store)
            face.activate(owner: Self.owner, epoch: 1) { _, _ in }
            try face.startDuplex()
            store.fails = true
            face.stop(owner: nil)
            XCTAssertFalse(io.isRunning)
            store.fails = false
            face.activate(owner: Self.owner, epoch: 2) { _, _ in }
            XCTAssertThrowsError(try face.startDuplex())
            XCTAssertFalse(io.isRunning)
        }
    }

    func testHostRejectsSharedLedgerWithoutOverwritingIt() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex()
            XCTAssertThrowsError(try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: store,
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [], clock: { 0 }, historyStore: store))
            XCTAssertEqual(store.bytes, [0xa0])
        }
    }

    func testHostHistoryInitializationFailureOccursAfterNativeSafetyStop() async throws {
        try await MainActor.run {
            for corrupt in [false, true] {
                let store = IntegrationLedger(), io = TestDuplex()
                store.fails = !corrupt
                if corrupt { store.bytes = [0xff] }
                try io.start()
                XCTAssertThrowsError(try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: IntegrationLedger(),
                    backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                    grants: [], clock: { 0 }, historyStore: store))
                XCTAssertFalse(io.isRunning)
            }
        }
    }

    func testUnknownHeardIsNotFoundAndSpeechStopRetainsDurablePrefix() async throws {
        try await MainActor.run {
            let store = IntegrationLedger(), io = TestDuplex(), tts = TestSynthesis()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            try face.configureHistory(store: store)
            face.activate(owner: Self.owner, epoch: 1) { _, _ in }
            XCTAssertThrowsError(try face.read(operation: "speech.heard", args: .record([("item", .text(Self.item))]), owner: Self.owner, now: 0)) { error in
                XCTAssertEqual((error as? NativeSurfaceError)?.code, "not_found")
            }
            _ = try face.begin(action: Self.speak(), epoch: 1, now: 0)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 4800), spans: [.init(endMS: 100, text: "hello")])))
            try face.tick(now: 1); io.renderedFrames = 2400
            _ = try face.begin(action: Self.stopAction(), epoch: 1, now: 2)
            let restored = try FaceHeardHistory(store: store)
            let heard = try restored.heard(item: Self.item, owner: Self.owner, epoch: 1)
            XCTAssertEqual(heard["text"], .text("hello"))
            XCTAssertEqual(heard["played_ms"], .unsigned(100))
            try face.speechStart(utterance: String(repeating: "4", count: 32), positionMS: 0)
            XCTAssertEqual(try FaceHeardHistory(store: store).heard(item: Self.item, owner: Self.owner, epoch: 1), heard)
        }
    }

    @MainActor private static func stopAction() -> CBORValue {
        .record([("id", .text(String(repeating: "3", count: 32))), ("coglet", .text(owner)),
            ("capability", .text("speech.stop")), ("deadline", .unsigned(2_000_000)),
            ("args", .record([("stream", .text(stream))]))])
    }

    @MainActor private static func speak() -> CBORValue {
        .record([("id", .text(item)), ("coglet", .text(owner)), ("capability", .text("speech.speak_chunk")),
            ("deadline", .unsigned(2_000_000)), ("args", .record([("stream", .text(stream)),
                ("chunk", .unsigned(0)), ("text", .text("hello")), ("final", .boolean(true))]))])
    }
}
