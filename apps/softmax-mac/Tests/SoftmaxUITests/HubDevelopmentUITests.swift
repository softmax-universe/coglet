import AppKit
import Darwin
import XCTest

/// Opt-in real application proof. Unlike snapshot smoke this MUST NOT pass
/// --ui-testing: it exercises the app's real developer store and helper.
@MainActor
final class HubDevelopmentUITests: XCTestCase {
    func testRealDevelopmentAppManualEnrollmentAndHubSideDrop() throws {
        let env = ProcessInfo.processInfo.environment
        guard let launcher = env["SOFTMAX_REAL_HUB_UI_LAUNCHER"] else {
            throw XCTSkip("Opt-in native development enrollment proof")
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("softmax-native-ui-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let app = XCUIApplication()
        app.launchEnvironment["SOFTMAX_DEVELOPMENT_HUB_DIRECTORY"] = directory.path
        app.launchArguments = []
        app.launch()
        defer { app.terminate() }
        let settings = app.windows["Softmax Settings"]
        func openHub() {
            app.activate()
            let open = app.buttons["Open Settings…"]
            if open.waitForExistence(timeout: 5) { open.click() }
            else {
                app.typeKey("k", modifierFlags: .command)
                app.textFields.firstMatch.typeText("Hub")
                app.typeKey(.return, modifierFlags: [])
            }
            XCTAssertTrue(settings.waitForExistence(timeout: 10))
            let hub = settings.buttons["Hub"]
            if hub.waitForExistence(timeout: 5) { hub.click() }
            else { XCTAssertTrue(settings.descendants(matching: .any)["hub.status"].waitForExistence(timeout: 5)) }
        }
        openHub()
        XCTAssertTrue(settings.descendants(matching: .any)["hub.development-warning"].waitForExistence(timeout: 5))
        settings.disclosureTriangles["Device identity"].coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5)).withOffset(CGVector(dx: 26, dy: 0)).click()
        func deviceID() throws -> String {
            let copy = settings.buttons["Copy device ID"]
            guard copy.waitForExistence(timeout: 5) else {
                throw NSError(domain: "device copy UI missing", code: 1)
            }
            copy.click()
            let candidate = NSPasteboard.general.string(forType: .string) ?? ""
            NSPasteboard.general.clearContents()
            guard candidate.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil else {
                throw NSError(domain: "invalid device UI", code: 1)
            }
            return candidate
        }
        let device = try deviceID()
        let mock = Process(), input = Pipe(), output = Pipe(), errors = Pipe()
        mock.executableURL = URL(fileURLWithPath: launcher)
        mock.arguments = [device]
        // XCTest injects architecture-specific runner libraries; do not inject
        // them into the standalone Rust executable (possibly under Rosetta).
        mock.environment = ["PATH": "/usr/bin:/bin", "HOME": NSHomeDirectory(),
                            "TMPDIR": FileManager.default.temporaryDirectory.path]
        mock.standardInput = input; mock.standardOutput = output
        mock.standardError = errors
        try mock.run()
        defer { try? input.fileHandleForWriting.close(); if mock.isRunning { mock.terminate() } }
        var bootstrap = Data()
        let deadline = Date().addingTimeInterval(15)
        while bootstrap.count < 16384 {
            var descriptor = pollfd(fd: output.fileHandleForReading.fileDescriptor, events: Int16(POLLIN), revents: 0)
            let remaining = max(0, Int32(deadline.timeIntervalSinceNow * 1000))
            guard remaining > 0, poll(&descriptor, 1, remaining) > 0 else { throw NSError(domain: "mock bootstrap timeout", code: 1) }
            guard let byte = try output.fileHandleForReading.read(upToCount: 1), !byte.isEmpty else {
                mock.waitUntilExit()
                let diagnostic = String(decoding: errors.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                    .replacingOccurrences(of: "[a-fA-F0-9]{64}", with: "<redacted-id>", options: .regularExpression)
                print("Mock exited \(mock.terminationStatus): \(diagnostic)")
                throw NSError(domain: "mock bootstrap", code: 1)
            }
            if byte == Data([10]) { break }
            bootstrap.append(byte)
        }
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: bootstrap) as? [String: Any])
        settings.buttons["Pair router…"].click()
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        let manual = sheet.disclosureTriangles["Developer: manual router address"]
        XCTAssertTrue(manual.waitForExistence(timeout: 5))
        manual.coordinate(withNormalizedOffset: CGVector(dx: 0.10, dy: 0.5)).click()
        sheet.textFields["Router name"].click()
        sheet.textFields["Router name"].typeText("Native development mock")
        sheet.textFields["Pinned router node ID (64 lowercase hex)"].click()
        sheet.textFields["Pinned router node ID (64 lowercase hex)"].typeText(try XCTUnwrap(fields["hub"] as? String))
        sheet.textFields["IP:port addresses, comma-separated"].click()
        sheet.textFields["IP:port addresses, comma-separated"].typeText(try XCTUnwrap(fields["addresses"] as? [String]).joined(separator: ","))
        sheet.scrollViews.firstMatch.scroll(byDeltaX: 0, deltaY: -180)
        sheet.secureTextFields["Invitation (64 lowercase hex, optional for enrolled device)"].click()
        sheet.secureTextFields["Invitation (64 lowercase hex, optional for enrolled device)"].typeText(try XCTUnwrap(fields["invite"] as? String))
        sheet.buttons["Connect router"].click()
        let connected = settings.descendants(matching: .any).matching(NSPredicate(format: "identifier == %@ AND (label == %@ OR value == %@)", "hub.status", "Router connected", "Router connected")).firstMatch
        XCTAssertTrue(connected.waitForExistence(timeout: 20))
        try input.fileHandleForWriting.write(contentsOf: Data("d".utf8))
        let offline = settings.descendants(matching: .any).matching(NSPredicate(format: "identifier == %@ AND (label == %@ OR value == %@)", "hub.status", "Router offline", "Router offline")).firstMatch
        // Query immediately: waitForExistence's initial one-second sampling can
        // miss the genuine offline state before the first automatic retry.
        let offlineDeadline = Date().addingTimeInterval(10)
        var sawOffline = false
        repeat {
            sawOffline = offline.exists
            if !sawOffline { Thread.sleep(forTimeInterval: 0.05) }
        } while !sawOffline && Date() < offlineDeadline
        XCTAssertTrue(sawOffline, "App must expose hub-side loss before automatic recovery")
        XCTAssertTrue(connected.waitForExistence(timeout: 35))
        app.terminate(); app.launch(); openHub()
        XCTAssertTrue(connected.waitForExistence(timeout: 20))
        settings.disclosureTriangles["Device identity"].coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5)).withOffset(CGVector(dx: 26, dy: 0)).click()
        // Avoid XCTest printing identity material in an equality failure.
        XCTAssertTrue(try deviceID() == device, "Persisted identity must survive relaunch")
        settings.disclosureTriangles["Device identity"].coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5)).withOffset(CGVector(dx: 26, dy: 0)).click()
        // Do not attach screenshots containing identity or invitation fields.
    }
}
