import XCTest
@testable import SoftmaxCore

final class StateMachineTests: XCTestCase {
    func testFirstLaunchRequiresOnboarding() {
        var machine = AppStateMachine(onboardingComplete: false)
        XCTAssertEqual(machine.state, .onboarding)
        XCTAssertFalse(machine.send(.beginListening))
        XCTAssertEqual(machine.state, .onboarding)
    }

    func testConversationAndBargeIn() {
        var machine = AppStateMachine(onboardingComplete: true)
        XCTAssertEqual(machine.state, .offline)
        let path: [(AppEvent, AppState)] = [
            (.hubConnected, .idle), (.beginListening, .listening),
            (.finishListening, .thinking), (.beginSpeaking, .speaking),
            (.beginListening, .listening), (.finishListening, .thinking),
            (.beginSpeaking, .speaking), (.finishSpeaking, .idle)
        ]
        for (event, expected) in path {
            XCTAssertTrue(machine.send(event))
            XCTAssertEqual(machine.state, expected)
        }
    }

    func testEveryStateEventPair() {
        let paths: [AppState: [AppEvent]] = [
            .onboarding: [], .offline: [.finishOnboarding],
            .idle: [.finishOnboarding, .hubConnected],
            .listening: [.finishOnboarding, .hubConnected, .beginListening],
            .thinking: [.finishOnboarding, .hubConnected, .beginListening, .finishListening],
            .speaking: [.finishOnboarding, .hubConnected, .beginListening, .finishListening, .beginSpeaking],
            .error: [.finishOnboarding, .fail]
        ]
        let allowed: [AppState: [AppEvent: AppState]] = [
            .onboarding: [.finishOnboarding: .offline],
            .offline: [.hubConnected: .idle, .fail: .error],
            .idle: [.beginListening: .listening, .hubDisconnected: .offline, .fail: .error],
            .listening: [.finishListening: .thinking, .cancel: .idle, .hubDisconnected: .offline, .fail: .error],
            .thinking: [.beginSpeaking: .speaking, .cancel: .idle, .hubDisconnected: .offline, .fail: .error],
            .speaking: [.beginListening: .listening, .finishSpeaking: .idle, .cancel: .idle,
                        .hubDisconnected: .offline, .fail: .error],
            .error: [.recover: .offline]
        ]
        for state in AppState.allCases {
            for event in AppEvent.allCases {
                var machine = AppStateMachine(onboardingComplete: false)
                for setup in paths[state]! { XCTAssertTrue(machine.send(setup)) }
                XCTAssertEqual(machine.state, state)
                let expected = allowed[state]?[event]
                XCTAssertEqual(machine.send(event), expected != nil, "\(state), \(event)")
                XCTAssertEqual(machine.state, expected ?? state)
            }
        }
    }

    func testRecoveryRequiresFreshConnection() {
        var machine = AppStateMachine(onboardingComplete: true)
        machine.send(.hubConnected)
        machine.send(.beginListening)
        machine.send(.hubDisconnected)
        XCTAssertFalse(machine.send(.finishListening))
        machine.send(.hubConnected)
        XCTAssertEqual(machine.state, .idle)
        machine.send(.fail)
        XCTAssertFalse(machine.send(.hubConnected))
        machine.send(.recover)
        XCTAssertEqual(machine.state, .offline)
    }

    func testEveryStateHasDistinctStatusSymbol() {
        XCTAssertEqual(Set(AppState.allCases.map(\.symbolName)).count, AppState.allCases.count)
    }
}
