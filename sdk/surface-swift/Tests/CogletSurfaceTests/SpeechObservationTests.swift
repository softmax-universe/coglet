import XCTest
import CogletTypes
@testable import CogletSurface

final class SpeechObservationTests: XCTestCase {
    private let item = String(repeating: "1", count: 32)
    private let owner = "coglet:" + String(repeating: "b", count: 64)
    private let other = "coglet:" + String(repeating: "c", count: 64)

    func testNativeItemAlignmentOwnerBound() throws {
        let projection = SpeechObservations()
        try projection.register(item: item, owner: owner, epoch: 1)
        try projection.append(item: item, owner: owner, epoch: 1, spans: [.init(endMS: 100, text: "heard"), .init(endMS: 200, text: " unheard")])
        try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 150, stopped: false)
        XCTAssertEqual(try projection.heard(item: item, owner: owner, epoch: 1)["text"], .text("heard"))
        XCTAssertThrowsError(try projection.heard(item: item, owner: other, epoch: 1))
        XCTAssertThrowsError(try projection.append(item: item, owner: owner, epoch: 2, spans: [.init(endMS: 300, text: "old callback")]))
        projection.revoke(owner: owner)
        XCTAssertThrowsError(try projection.heard(item: item, owner: owner, epoch: 1))
    }
    func testNativeAlignmentOnlyAppendsSpans() throws {
        let projection = SpeechObservations()
        try projection.register(item: item, owner: owner, epoch: 1)
        try projection.append(item: item, owner: owner, epoch: 1, spans: [.init(endMS: 100, text: "e\u{301}")])
        XCTAssertThrowsError(try projection.append(item: item, owner: owner, epoch: 1, spans: [.init(endMS: 99, text: "rewrite")]))
        try projection.append(item: item, owner: owner, epoch: 1, spans: [.init(endMS: 200, text: "é")])
        try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 200, stopped: false)
        XCTAssertEqual(try projection.heard(item: item, owner: owner, epoch: 1)["text"], .text("e\u{301}é"))
        XCTAssertThrowsError(try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 199, stopped: false))
    }
    func testStoppedItemAlignmentFrozen() throws {
        let projection = SpeechObservations()
        try projection.register(item: item, owner: owner, epoch: 1)
        try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 50, stopped: true)
        XCTAssertEqual(try projection.heard(item: item, owner: owner, epoch: 1)["text"], .text(""))
        XCTAssertThrowsError(try projection.append(item: item, owner: owner, epoch: 1, spans: [.init(endMS: 40, text: "invented")]))
        XCTAssertThrowsError(try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 60, stopped: false))
        try projection.playback(item: item, owner: owner, epoch: 1, playedMS: 50, stopped: true)
    }
    func testTranscriptRevisionsFinalAndUTF8ByteCap() throws {
        let normalizer = SpeechObservations()
        try normalizer.start(utterance: item)
        XCTAssertTrue(try normalizer.transcript(utterance: item, revision: 1, text: "first", final: false))
        XCTAssertFalse(try normalizer.transcript(utterance: item, revision: 1, text: "stale", final: false))
        XCTAssertTrue(try normalizer.transcript(utterance: item, revision: 2, text: "final", final: true))
        XCTAssertFalse(try normalizer.transcript(utterance: item, revision: 3, text: "late", final: false))
        XCTAssertThrowsError(try normalizer.start(utterance: item))
        let second = String(repeating: "2", count: 32)
        try normalizer.start(utterance: second)
        XCTAssertThrowsError(try normalizer.transcript(utterance: second, revision: 1, text: String(repeating: "🦀", count: 4097), final: true))
        XCTAssertTrue(try normalizer.transcript(utterance: second, revision: 1, text: String(repeating: "🦀", count: 4096), final: true))
    }
}
