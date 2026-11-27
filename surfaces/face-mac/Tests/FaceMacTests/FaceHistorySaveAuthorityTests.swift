import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class AuthorityLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var onSave: (() -> Void)?
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes; onSave?() }
}
@MainActor private final class AuthorityClock { var now: UInt64 = 0 }

/// A blocking heard-history save must never carry stale deadline or grant
/// authority into a provider call or audio effect.
final class FaceHistorySaveAuthorityTests: XCTestCase {
    private static let owner = "coglet:" + String(repeating: "a", count: 64)
    private static let identity = "surface:" + String(repeating: "b", count: 64)
    enum Phase { case registration, alignment, queuedNext }

    @MainActor private static func act(host: FaceSurfaceHost, id: String, chunk: UInt64, final: Bool, deadline: UInt64) throws {
        let action = CBORValue.record([("id", .text(id)), ("coglet", .text(owner)), ("capability", .text("speech.speak_chunk")),
            ("deadline", .unsigned(deadline)), ("correlation_id", .text(String(repeating: "f", count: 32))),
            ("args", .record([("stream", .text(String(repeating: "a", count: 32))), ("chunk", .unsigned(chunk)),
                              ("text", .text("hello")), ("final", .boolean(final))]))])
        try host.receive(SurfaceWire.encode(.message("act", .record([("grant_epoch", .unsigned(host.grantEpoch)),
            ("action", action)])), from: .runtime))
    }

    /// Returns (synthesis calls, enqueues, running) after the save advances time.
    @MainActor private static func run(_ phase: Phase, expireGrant: Bool, advance: Bool) throws -> (Int, Int, Bool) {
        let clock = AuthorityClock(), io = TestDuplex(), tts = TestSynthesis(), history = AuthorityLedger()
        let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: AuthorityLedger(),
            backend: FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }),
            grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)).union(["speech.heard"]),
                           resources: ["audio.mic", "audio.speaker", "display.face"],
                           expires: expireGrant ? 500 : 10_000_000)], clock: { clock.now }, historyStore: history)
        try host.start()
        let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
        try host.receive(SurfaceWire.encode(.message("hello_reply", .record([("selection", .record([("version", .text("0.1"))])),
            ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
        _ = host.drain(); try host.activateDuplex(localConsent: true)
        let item0 = String(repeating: "1", count: 32), item1 = String(repeating: "3", count: 32)
        let jump = { if advance { clock.now = 1000 } }
        let deadline: UInt64 = phase == .queuedNext ? 5000 : (expireGrant ? 2000 : 1000)
        let spans: [SpeechObservations.Span] = [.init(endMS: 100, text: "hello")]
        let audio = FaceSynthesizedAudio(samples: Array(repeating: 1, count: 2400), spans: spans)
        if phase == .registration {
            history.onSave = jump // fires inside begin's register save
            try? Self.act(host: host, id: item0, chunk: 0, final: true, deadline: deadline)
        } else {
            try Self.act(host: host, id: item0, chunk: 0, final: phase == .alignment, deadline: deadline)
            if phase == .queuedNext {
                try Self.act(host: host, id: item1, chunk: 1, final: true, deadline: expireGrant ? 2000 : 1000)
            }
            tts.callbacks[0](.success(audio))
            if phase == .alignment { history.onSave = jump }
            try? host.pump() // alignment save, then audio start/enqueue
            if phase == .queuedNext {
                XCTAssertEqual(io.enqueued.count, 1)
                io.renderedFrames = 2400 // chunk 0 plays out fully
                history.onSave = jump // fires inside chunk 0's playback save
                try? host.pump() // completes chunk 0; next chunk synthesis must reauthorize
            }
        }
        let result = (tts.callbacks.count, io.enqueued.count, io.isRunning)
        history.onSave = nil; try? host.close()
        return result
    }

    @MainActor private static func check(_ phase: Phase) throws {
        let control = try Self.run(phase, expireGrant: false, advance: false)
        switch phase {
        case .registration: XCTAssertEqual(control.0, 1); XCTAssertTrue(control.2)
        case .alignment: XCTAssertEqual(control.1, 1); XCTAssertTrue(control.2)
        case .queuedNext: XCTAssertEqual(control.0, 2); XCTAssertEqual(control.1, 1)
        }
        for grantExpiry in [false, true] {
            let r = try Self.run(phase, expireGrant: grantExpiry, advance: true)
            let label = "\(phase) grantExpiry=\(grantExpiry)"
            switch phase {
            case .registration: XCTAssertEqual(r.0, 0, "synthesis started: " + label); XCTAssertEqual(r.1, 0)
            case .alignment: XCTAssertEqual(r.1, 0, "audio enqueued: " + label); XCTAssertFalse(r.2, label)
            case .queuedNext: XCTAssertEqual(r.0, 1, "next chunk synthesized: " + label); XCTAssertEqual(r.1, 1)
            }
            if grantExpiry { XCTAssertFalse(r.2, label) }
        }
    }

    func testRegistrationSaveCannotStartSynthesisAfterDeadlineOrGrantExpiry() async throws {
        try await MainActor.run { try Self.check(.registration) }
    }
    func testAlignmentSaveCannotStartOrEnqueueAudioAfterDeadlineOrGrantExpiry() async throws {
        try await MainActor.run { try Self.check(.alignment) }
    }
    func testPlaybackSaveCannotSynthesizeQueuedNextChunkAfterDeadlineOrGrantExpiry() async throws {
        try await MainActor.run { try Self.check(.queuedNext) }
    }
    func testDirectBackendWithoutHostKeepsCompatibility() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis(), store = AuthorityLedger()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            try face.configureHistory(store: store)
            face.activate(owner: Self.owner, epoch: 1) { _, _ in }
            let action = CBORValue.record([("id", .text(String(repeating: "1", count: 32))), ("coglet", .text(Self.owner)),
                ("capability", .text("speech.speak_chunk")), ("deadline", .unsigned(1000)),
                ("correlation_id", .text(String(repeating: "f", count: 32))),
                ("args", .record([("stream", .text(String(repeating: "a", count: 32))), ("chunk", .unsigned(0)),
                                  ("text", .text("hello")), ("final", .boolean(true))]))])
            _ = try face.begin(action: action, epoch: 1, now: 0)
            XCTAssertEqual(tts.callbacks.count, 1)
            tts.callbacks[0](.success(.init(samples: Array(repeating: 1, count: 2400), spans: [.init(endMS: 100, text: "hello")])))
            try face.tick(now: 1)
            XCTAssertEqual(io.enqueued.count, 1)
        }
    }
}
