#if canImport(AppKit)
import AppKit
import XCTest
@testable import SoftmaxAppSurface

@MainActor
final class NativeFocusObservationTests: XCTestCase {
    private func window() -> NSWindow {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 160),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        return window
    }

    // No synthetic focus or key-window assumptions: this tests actual AppKit
    // attachment/visibility only. SwiftUI focus remains covered by the unchanged
    // app UI wire proof, not inferred from this predicate test.
    func testActualAttachedVisibleSheetSupersedesOnlyItsParentUntilDismissal() async throws {
        _ = NSApplication.shared
        let parent = window(), sheet = window(), unrelated = window()
        parent.animationBehavior = .none
        sheet.animationBehavior = .none
        parent.orderFrontRegardless()
        unrelated.orderFrontRegardless()
        defer {
            if sheet.sheetParent != nil { parent.endSheet(sheet) }
            sheet.orderOut(nil)
            sheet.close(); parent.close(); unrelated.close()
        }
        XCTAssertTrue(parent.isVisible)
        XCTAssertNil(parent.attachedSheet)
        XCTAssertFalse(sheet.isVisible)
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: parent))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: sheet))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: nil))

        parent.beginSheet(sheet, completionHandler: nil)
        let shownDeadline = ContinuousClock.now.advanced(by: .seconds(2))
        while !(parent.attachedSheet === sheet && sheet.sheetParent === parent && sheet.isVisible),
              ContinuousClock.now < shownDeadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(parent.attachedSheet === sheet, "Must exercise real beginSheet attachment")
        XCTAssertTrue(sheet.sheetParent === parent)
        XCTAssertTrue(sheet.isVisible, "An unshown sheet cannot exercise supersession")
        XCTAssertFalse(NativeFocusObservation.isEligible(owner: parent))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: sheet))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: unrelated))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: nil))

        parent.endSheet(sheet)
        sheet.orderOut(nil)
        let dismissedDeadline = ContinuousClock.now.advanced(by: .seconds(2))
        while (parent.attachedSheet != nil || sheet.sheetParent != nil || sheet.isVisible),
              ContinuousClock.now < dismissedDeadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertNil(parent.attachedSheet, "Must exercise real endSheet detachment")
        XCTAssertNil(sheet.sheetParent)
        XCTAssertFalse(sheet.isVisible)
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: parent))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: unrelated))
        XCTAssertTrue(NativeFocusObservation.isEligible(owner: nil))
    }
}
#endif
