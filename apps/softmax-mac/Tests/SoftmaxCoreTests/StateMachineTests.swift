import XCTest
@testable import SoftmaxCore

final class StateMachineTests: XCTestCase {
    func testFirstLaunchRequiresOnboarding() {
        let machine = AppStateMachine(onboardingComplete: false)
        XCTAssertEqual(machine.state, .onboarding)
        XCTAssertFalse(machine.send(.beginListening))
        XCTAssertEqual(machine.state, .onboarding)
    }
}
