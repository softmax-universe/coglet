import AppKit
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
        let app = XCUIApplication()
        app.launchArguments = []
        app.launch()
        defer { app.terminate() }
        let settings = app.windows["Softmax Settings"]
        func openHub() {
            let open = app.buttons["Open Settings…"]
            if open.waitForExistence(timeout: 5) { open.click() }
            else {
                app.typeKey("k", modifierFlags: .command)
                app.textFields.firstMatch.typeText("Hub")
                app.typeKey(.return, modifierFlags: [])
            }
            XCTAssertTrue(settings.waitForExistence(timeout: 10))
            settings.buttons["Hub"].click()
        }
        openHub()
        XCTAssertTrue(settings.staticTexts.containing(NSPredicate(format: "label BEGINSWITH %@", "Development identity")).firstMatch.exists)
        settings.disclosureTriangles["Device identity"].click()
        let deviceText = settings.staticTexts.matching(NSPredicate(format: "label MATCHES %@", "[a-f0-9]{64}")).firstMatch
        XCTAssertTrue(deviceText.waitForExistence(timeout: 10))
        let device = deviceText.label
        settings.disclosureTriangles["Device identity"].click()
        let mock = Process(), input = Pipe(), output = Pipe()
        mock.executableURL = URL(fileURLWithPath: launcher)
        mock.arguments = [device]
        mock.standardInput = input; mock.standardOutput = output
        mock.standardError = FileHandle.nullDevice
        try mock.run()
        defer { try? input.fileHandleForWriting.close(); if mock.isRunning { mock.terminate() } }
        var bootstrap = Data()
        while bootstrap.count < 16384 {
            guard let byte = try output.fileHandleForReading.read(upToCount: 1), !byte.isEmpty else { throw NSError(domain: "mock bootstrap", code: 1) }
            if byte == Data([10]) { break }
            bootstrap.append(byte)
        }
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: bootstrap) as? [String: Any])
        settings.buttons["Pair router…"].click()
        let sheet = settings.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        sheet.disclosureTriangles["Developer: manual router address"].click()
        sheet.textFields["Router name"].typeText("Native development mock")
        sheet.textFields["Pinned router node ID (64 lowercase hex)"].typeText(try XCTUnwrap(fields["hub"] as? String))
        sheet.textFields["IP:port addresses, comma-separated"].typeText(try XCTUnwrap(fields["addresses"] as? [String]).joined(separator: ","))
        sheet.secureTextFields["Invitation (64 lowercase hex, optional for enrolled device)"].typeText(try XCTUnwrap(fields["invite"] as? String))
        sheet.buttons["Connect router"].click()
        let connected = settings.staticTexts["Router connected"]
        XCTAssertTrue(connected.waitForExistence(timeout: 20))
        try input.fileHandleForWriting.write(contentsOf: Data("d".utf8))
        XCTAssertTrue(settings.staticTexts["Router offline"].waitForExistence(timeout: 10))
        XCTAssertTrue(connected.waitForExistence(timeout: 35))
        app.terminate(); app.launch(); openHub()
        XCTAssertTrue(connected.waitForExistence(timeout: 20))
        settings.disclosureTriangles["Device identity"].click()
        XCTAssertEqual(settings.staticTexts.matching(NSPredicate(format: "label MATCHES %@", "[a-f0-9]{64}")).firstMatch.label, device)
        settings.disclosureTriangles["Device identity"].click()
        // Do not attach screenshots containing identity or invitation fields.
    }
}
