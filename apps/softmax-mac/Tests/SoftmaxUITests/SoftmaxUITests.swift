import XCTest

final class SoftmaxUITests: XCTestCase {
    @MainActor
    func testLaunchAndOpenSettings() {
        let app = XCUIApplication()
        // Fresh, isolated onboarding state; never touches production preferences or TCC.
        app.launchArguments = ["--ui-testing"]
        app.launch()
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        for tab in ["General", "Hotkey", "Audio", "Hub", "Coglets", "Updates", "Privacy"] {
            XCTAssertTrue(settings.buttons[tab].exists, "Missing settings tab: \(tab)")
        }
        settings.buttons["Updates"].click()
        let check = settings.buttons["Check for Updates…"]
        XCTAssertTrue(check.exists)
        XCTAssertFalse(check.isEnabled, "UI-test/development builds must not start updater requests")
        let automatic = settings.checkBoxes["Automatically check for updates"]
        XCTAssertTrue(automatic.exists)
        XCTAssertFalse(automatic.isEnabled)
        app.terminate()
    }
}
