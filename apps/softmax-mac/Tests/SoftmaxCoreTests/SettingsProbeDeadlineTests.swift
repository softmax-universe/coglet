#if DEBUG
import XCTest
@testable import SoftmaxCore

final class SettingsProbeDeadlineTests: XCTestCase {
    func testDeadlineBoundaryAndTickBudget() {
        XCTAssertFalse(SettingsProbeDeadline.expired(now: 104.999, start: 100, ticks: 500))
        XCTAssertTrue(SettingsProbeDeadline.expired(now: 105, start: 100, ticks: 500))
        XCTAssertTrue(SettingsProbeDeadline.expired(now: 100.001, start: 100, ticks: 501))
    }

    func testLateTruePredicateIsNeverEvaluatedOrAccepted() {
        var evaluated = false
        if !SettingsProbeDeadline.expired(now: 105.1, start: 100, ticks: 1) {
            evaluated = true // The predicate would already be true after a stalled run loop.
        }
        XCTAssertFalse(evaluated)
    }

    func testDispatchThatCrossesDeadlineCannotBeAccepted() {
        XCTAssertFalse(SettingsProbeDeadline.expired(now: 104.9, start: 100, ticks: 3))
        // A synchronous native dispatch returns after the bound, even though it completed.
        XCTAssertTrue(SettingsProbeDeadline.expired(now: 105.1, start: 100, ticks: 3))
    }

    func testInvalidMonotonicTimesRefuse() {
        XCTAssertTrue(SettingsProbeDeadline.expired(now: .nan, start: 100, ticks: 0))
        XCTAssertTrue(SettingsProbeDeadline.expired(now: .infinity, start: 100, ticks: 0))
        XCTAssertTrue(SettingsProbeDeadline.expired(now: 99, start: 100, ticks: 0))
    }
}
#endif
