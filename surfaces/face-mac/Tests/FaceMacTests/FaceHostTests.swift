import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class HostLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var onSave: (() -> Void)?
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes; onSave?() }
}
@MainActor private final class HostClock { var now: UInt64 = 0 }

@MainActor private final class TestControlLink: FaceControlLink {
    var sent: [[UInt8]] = []
    var incoming: [[UInt8]] = []
    var blocked = true
    var closed = false
    var failReceive = false
    func send(_ bytes: [UInt8]) throws -> Bool {
        if blocked { return false }; sent.append(bytes); return true
    }
    func receive() throws -> [UInt8]? {
        if failReceive { throw SurfaceFailure.closed }
        return incoming.isEmpty ? nil : incoming.removeFirst()
    }
    func flush() throws -> Bool { !blocked }
    func close() { closed = true }
}

final class FaceHostTests: XCTestCase {
    func testAdmissionResamplesAfterDurableSaveBeforeSynthesis() async throws {
        try await MainActor.run {
            for grantExpiry in [false, true] {
                let clock = HostClock(), store = HostLedger(), tts = TestSynthesis()
                let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: store,
                    backend: FaceSurfaceBackend(audio: TestDuplex(), synthesis: tts, renderer: .init { _ in }),
                    grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: grantExpiry ? 500 : 10_000_000)], clock: { clock.now })
                try Self.negotiate(host); try host.activateDuplex(localConsent: true)
                store.onSave = { clock.now = 1000 }
                try host.receive(Self.speak(host: host, suffix: "1", chunk: 0, deadline: grantExpiry ? 2000 : 1000))
                XCTAssertTrue(tts.callbacks.isEmpty, "expired authority must never initiate synthesis")
                store.onSave = nil; try host.close()
            }
        }
    }
    func testLateFinalBeforeTimerCannotPublishPrivateTranscript() async throws {
        try await MainActor.run {
            let clock = HostClock(), recognition = TestRecognition()
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 100_000_000)], clock: { clock.now })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            _ = try host.beginCapture(recognition: recognition, positionMS: 0); _ = host.drain()
            host.finishCapture(); clock.now = 5_000_000
            try recognition.callbacks[0](.init(revision: 1, text: "too late private", final: true, positionMS: 100))
            let messages = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(messages.count, 1)
            XCTAssertEqual(messages.first?["body"]?["payload"]?["text"], .text(""))
            try host.close()
        }
    }
    func testOffSwitchNotifiesWritableAttachedPeerAfterNativeStop() async throws {
        try await MainActor.run {
            let io = TestDuplex(), link = TestControlLink(); link.blocked = false
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: link); try host.start()
            let hello = try SurfaceWire.decode(link.sent[0], from: .surface)
            link.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            try host.pump(); try host.activateDuplex(localConsent: true); link.sent = []
            try host.localOffSwitch()
            XCTAssertFalse(io.isRunning); XCTAssertTrue(link.closed)
            let messages = try link.sent.map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertTrue(messages.contains { $0["type"] == .text("kill") })
            XCTAssertTrue(messages.contains { $0["type"] == .text("health") && $0["body"]?["state"] == .text("stopped") })
            try host.close()
        }
    }
    func testAttachedControlLinkRetriesIdenticalHelloAndDisconnectsNativeOnTransportFailure() async throws {
        try await MainActor.run {
            let io = TestDuplex(), recognition = TestRecognition(), link = TestControlLink()
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: link)
            try host.start(); try host.pump()
            XCTAssertTrue(link.sent.isEmpty)
            XCTAssertTrue(host.drain().isEmpty, "attached transport has one output consumer")
            link.blocked = false; try host.pump()
            XCTAssertEqual(link.sent.count, 1)
            let hello = try SurfaceWire.decode(link.sent[0], from: .surface)
            link.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            try host.pump(); link.sent = []
            try host.activateDuplex(localConsent: true)
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try host.pump()
            let events = try link.sent.map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(events.first?["body"]?["kind"], .text("speech.start"))
            link.failReceive = true
            XCTAssertThrowsError(try host.pump())
            XCTAssertTrue(link.closed)
            XCTAssertFalse(io.isRunning)
            XCTAssertTrue(recognition.inputs[0].cancelled)
            try host.close()
        }
    }
    func testGrantFenceDropsBlockedPrivateTransportBacklogAndStopsNative() async throws {
        try await MainActor.run {
            let io = TestDuplex(), recognition = TestRecognition(), link = TestControlLink()
            link.blocked = false
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: link); try host.start()
            let hello = try SurfaceWire.decode(link.sent[0], from: .surface)
            link.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            try host.pump(); link.sent = []
            try host.activateDuplex(localConsent: true)
            link.blocked = true
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try recognition.callbacks[0](.init(revision: 1, text: "private backlog", final: false, positionMS: 10))
            try host.pump()
            XCTAssertTrue(link.sent.isEmpty)
            try host.installGrants([])
            link.blocked = false
            XCTAssertThrowsError(try host.pump())
            XCTAssertTrue(link.closed)
            XCTAssertTrue(link.sent.isEmpty)
            XCTAssertFalse(io.isRunning)
            try host.close()
        }
    }
    func testSameEpochReactivationCannotReleaseBlockedObservationBacklog() async throws {
        try await MainActor.run {
            let io = TestDuplex(), recognition = TestRecognition(), link = TestControlLink()
            link.blocked = false
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: link); try host.start()
            let hello = try SurfaceWire.decode(link.sent[0], from: .surface)
            link.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            try host.pump(); link.sent = []
            try host.activateDuplex(localConsent: true)
            link.blocked = true
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try recognition.callbacks[0](.init(revision: 1, text: "private backlog", final: false, positionMS: 10))
            try host.pump()
            XCTAssertTrue(link.sent.isEmpty)
            XCTAssertThrowsError(try host.activateDuplex(localConsent: true))
            XCTAssertTrue(link.closed)
            XCTAssertTrue(link.sent.isEmpty)
            XCTAssertFalse(io.isRunning)
            try host.close()
        }
    }
    private static let owner = "coglet:" + String(repeating: "a", count: 64)
    private static let identity = "surface:" + String(repeating: "b", count: 64)
    func testVoiceActivityUsesCapturedMediaPositionNotHostElapsedTime() async throws {
        try await MainActor.run {
            let clock = HostClock(), io = TestDuplex(), recognition = TestRecognition()
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            try host.enableVoiceDetection(recognition: recognition)
            clock.now = 1_000_000
            io.captured = [Int16](repeating: 0, count: 2400); try host.pump(); _ = host.drain()
            io.onVoiceActivity?(true)
            let event = try SurfaceWire.decode(host.drain()[0], from: .surface)
            XCTAssertEqual(event["body"]?["payload"]?["position_ms"], .unsigned(100))
            try host.close()
        }
    }
    func testNativeVoiceActivityStartsAndEndsCaptureAndOldHookCannotRebind() async throws {
        try await MainActor.run {
            let io = TestDuplex(), recognition = TestRecognition()
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            try host.enableVoiceDetection(recognition: recognition)
            let oldHook = io.onVoiceActivity
            oldHook?(true)
            XCTAssertEqual(recognition.inputs.count, 1)
            oldHook?(false)
            XCTAssertTrue(recognition.inputs[0].finished)
            try host.activateDuplex(localConsent: true)
            oldHook?(true)
            XCTAssertEqual(recognition.inputs.count, 1, "old VAD generation must not create capture under the newest token")
            try host.close()
        }
    }
    func testCaptureGraphFailureDisconnectsEvenWithNoPlaybackItems() async throws {
        try await MainActor.run {
            let io = TestDuplex()
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            io.captureFailed = true
            XCTAssertThrowsError(try host.pump())
            XCTAssertFalse(io.isRunning)
            XCTAssertTrue(host.drain().isEmpty)
            try host.close()
        }
    }
    func testHostCapturePublishesWireFinalAndRevocationFencesLateTranscript() async throws {
        try await MainActor.run {
            let clock = HostClock(), io = TestDuplex(), recognition = TestRecognition()
            let backend = FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in })
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(), backend: backend,
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            try Self.negotiate(host)
            XCTAssertThrowsError(try host.beginCapture(recognition: recognition, positionMS: 0))
            try host.activateDuplex(localConsent: true)
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try recognition.callbacks[0](.init(revision: 1, text: "hello", final: true, positionMS: 100))
            let events = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(events.map { $0["body"]?["kind"] }, [.text("speech.start"), .text("speech.end")])
            _ = try host.beginCapture(recognition: recognition, positionMS: 200)
            try host.installGrants([]); _ = host.drain()
            try recognition.callbacks[1](.init(revision: 1, text: "private late", final: true, positionMS: 300))
            XCTAssertTrue(host.drain().isEmpty)
            XCTAssertTrue(recognition.inputs[1].cancelled)
            try host.close()
        }
    }
    func testRestartRetainsAdmittedStreamTombstoneWithoutReplayingAudio() async throws {
        try await MainActor.run {
            let clock = HostClock(), store = HostLedger(), io = TestDuplex(), tts = TestSynthesis()
            @MainActor func make() throws -> FaceSurfaceHost {
                try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: store,
                    backend: FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }),
                    grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            }
            let host = try make(); try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            try host.receive(Self.speak(host: host, suffix: "1", chunk: 0, deadline: 1000))
            try host.close()
            let reopened = try make(); try Self.negotiate(reopened); try reopened.activateDuplex(localConsent: true)
            try reopened.receive(Self.speak(host: reopened, suffix: "2", chunk: 0, deadline: 1000))
            let reply = try SurfaceWire.decode(reopened.drain()[0], from: .surface)
            XCTAssertEqual(reply["body"]?["accepted"], .boolean(false))
            XCTAssertEqual(reply["body"]?["error"]?["code"], .text("conflict"))
            XCTAssertEqual(tts.callbacks.count, 1)
            XCTAssertTrue(io.enqueued.isEmpty)
            try reopened.close()
        }
    }
    func testRestartDoesNotTurnRejectedSpeechIntoAStreamTombstone() async throws {
        try await MainActor.run {
            let clock = HostClock(), store = HostLedger(), io = TestDuplex(), tts = TestSynthesis()
            @MainActor func make() throws -> FaceSurfaceHost {
                try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: store,
                    backend: FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }),
                    grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            }
            let rejected = try make(); try Self.negotiate(rejected)
            try rejected.receive(Self.speak(host: rejected, suffix: "1", chunk: 0, deadline: 1000))
            let first = try SurfaceWire.decode(rejected.drain()[0], from: .surface)
            XCTAssertEqual(first["body"]?["accepted"], .boolean(false))
            try rejected.close()
            let reopened = try make(); try Self.negotiate(reopened); try reopened.activateDuplex(localConsent: true)
            try reopened.receive(Self.speak(host: reopened, suffix: "2", chunk: 0, deadline: 1000))
            let second = try SurfaceWire.decode(reopened.drain()[0], from: .surface)
            XCTAssertEqual(second["body"]?["accepted"], .boolean(true))
            XCTAssertEqual(tts.callbacks.count, 1)
            try reopened.close()
        }
    }
    func testActionExpiryCancelsLaterQueueWithoutStartingAudio() async throws {
        try await MainActor.run {
            let clock = HostClock(), io = TestDuplex(), tts = TestSynthesis()
            let backend = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(), backend: backend,
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            let first = try Self.speak(host: host, suffix: "1", chunk: 0, deadline: 1000)
            try host.receive(first)
            try host.receive(Self.speak(host: host, suffix: "2", chunk: 1, deadline: 2000))
            _ = host.drain()
            tts.callbacks[0](.success(.init(samples: [1, 2, 3])))
            clock.now = 1000; try host.pump()
            let values = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            let outcomes = values.compactMap { $0["body"]?["payload"] }.filter { $0["status"] != nil }
            XCTAssertEqual(Set(outcomes.compactMap { $0["status"]?.faceText }), ["expired", "cancelled"])
            XCTAssertTrue(io.enqueued.isEmpty)
            try host.receive(first)
            let retries = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(retries.last?["body"]?["payload"]?["status"], .text("expired"))
            try host.close()
        }
    }
    @MainActor private static func negotiate(_ host: FaceSurfaceHost) throws {
        try host.start()
        let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
        try host.receive(SurfaceWire.encode(.message("hello_reply", .record([
            ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
        _ = host.drain()
    }
    @MainActor private static func speak(host: FaceSurfaceHost, suffix: String, chunk: UInt64, deadline: UInt64) throws -> [UInt8] {
        try SurfaceWire.encode(.message("act", .record([("grant_epoch", .unsigned(host.grantEpoch)), ("action", .record([
            ("id", .text(String(repeating: "0", count: 31) + suffix)), ("coglet", .text(owner)),
            ("capability", .text("speech.speak_chunk")), ("deadline", .unsigned(deadline)),
            ("correlation_id", .text(String(repeating: "f", count: 32))),
            ("args", .record([("stream", .text(String(repeating: "a", count: 32))), ("chunk", .unsigned(chunk)),
                              ("text", .text("hello")), ("final", .boolean(false))]))]))])), from: .runtime)
    }
    func testHostRequiresLocalConsentAndCommonDuplexLeaseBeforeStartingGraph() async throws {
        try await MainActor.run {
            let io = TestDuplex(), backend = FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in })
            let clock = HostClock()
            let operations = Set(FaceSurfaceHost.operations.map(\.name))
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(), backend: backend,
                grants: [.init(coglet: Self.owner, operations: operations, resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)],
                clock: { clock.now })
            try host.start()
            let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
            try host.receive(SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            _ = host.drain()
            XCTAssertThrowsError(try host.activateDuplex(localConsent: false))
            XCTAssertFalse(io.isRunning)
            try host.activateDuplex(localConsent: true)
            XCTAssertTrue(io.isRunning)
            try host.installGrants([.init(coglet: Self.owner, operations: operations, resources: ["audio.speaker", "display.face"], expires: 10_000_000)])
            XCTAssertFalse(io.isRunning)
            XCTAssertThrowsError(try host.activateDuplex(localConsent: true))
            try host.close()
        }
    }
}
