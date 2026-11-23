import XCTest
@testable import SoftmaxCore

final class PermissionTests: XCTestCase {
    func testMicrophoneAuthorizationMappingIncludingUnknown() {
        XCTAssertEqual(PermissionStatus.microphone(rawValue: 0), .notDetermined)
        XCTAssertEqual(PermissionStatus.microphone(rawValue: 1), .restricted)
        XCTAssertEqual(PermissionStatus.microphone(rawValue: 2), .denied)
        XCTAssertEqual(PermissionStatus.microphone(rawValue: 3), .granted)
        XCTAssertEqual(PermissionStatus.microphone(rawValue: -1), .unknown)
        XCTAssertEqual(PermissionStatus.microphone(rawValue: 99), .unknown)
    }

    func testInputMonitoringDoesNotInventDenial() {
        XCTAssertEqual(PermissionStatus.inputMonitoring(granted: true), .granted)
        XCTAssertEqual(PermissionStatus.inputMonitoring(granted: false), .notGranted)
    }

    func testCaptureReadinessRequiresBothPermissions() {
        for mic in PermissionStatus.allCases {
            for input in PermissionStatus.allCases {
                let permissions = PermissionSnapshot(microphone: mic, inputMonitoring: input)
                XCTAssertEqual(permissions.canUseGlobalPushToTalk, mic == .granted && input == .granted)
            }
        }
    }

    func testLoginItemMappingAndUnknown() {
        XCTAssertEqual(LoginItemStatus(serviceRawValue: 0), .disabled)
        XCTAssertEqual(LoginItemStatus(serviceRawValue: 1), .enabled)
        XCTAssertEqual(LoginItemStatus(serviceRawValue: 2), .requiresApproval)
        XCTAssertEqual(LoginItemStatus(serviceRawValue: 3), .unavailable)
        XCTAssertEqual(LoginItemStatus(serviceRawValue: 99), .unknown)
    }
}
