import Foundation
#if canImport(CoreGraphics)
import CoreGraphics
#endif
import XCTest
@testable import SoftmaxCore

final class OrbTests: XCTestCase {
    func testTogglePersistsAcrossInstancesAndDefaultsOff() throws {
        let suite = "OrbTests-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let first = OrbPreferences(defaults: defaults)
        XCTAssertFalse(first.isEnabled)
        first.isEnabled = true
        XCTAssertTrue(OrbPreferences(defaults: try XCTUnwrap(UserDefaults(suiteName: suite))).isEnabled)
        first.isEnabled = false
        XCTAssertFalse(OrbPreferences(defaults: defaults).isEnabled)
    }

    func testPanelStaysOnVisibleScreenAndTinyScreenSnapIsDeterministic() {
        let screen = CGRect(x: 0, y: 0, width: 1280, height: 800)
        let frame = OrbGeometry.chatFrame(orb: CGRect(x: 1196, y: 716, width: 68, height: 68), in: screen)
        XCTAssertTrue(screen.contains(frame))
        XCTAssertEqual(frame.size, CGSize(width: 408, height: 548))
        XCTAssertEqual(OrbGeometry.snapped(.zero, in: CGRect(x: 0, y: 0, width: 50, height: 50)),
                       CGPoint(x: 16, y: 16))
    }

    func testDeniedAndRestrictedPermissionsHaveActionableGuidance() {
        XCTAssertTrue(PermissionStatus.denied.microphoneGuidance?.contains("System Settings") == true)
        XCTAssertTrue(PermissionStatus.restricted.microphoneGuidance?.contains("administrator") == true)
        XCTAssertNil(PermissionStatus.granted.microphoneGuidance)
    }

    func testSnapsToNearestHorizontalEdgeAndClampsVertically() {
        let screen = CGRect(x: -1440, y: 40, width: 1440, height: 860)
        XCTAssertEqual(OrbGeometry.snapped(CGPoint(x: -1400, y: -50), in: screen),
                       CGPoint(x: -1424, y: 56))
        XCTAssertEqual(OrbGeometry.snapped(CGPoint(x: -200, y: 1000), in: screen),
                       CGPoint(x: -84, y: 816))
        XCTAssertEqual(OrbGeometry.snapped(CGPoint(x: -100, y: 250), in: screen),
                       CGPoint(x: -84, y: 250))
    }
}
