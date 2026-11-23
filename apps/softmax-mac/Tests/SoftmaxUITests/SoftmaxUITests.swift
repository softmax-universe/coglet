import XCTest

final class SoftmaxUITests: XCTestCase {
    @MainActor
    func testOrbToggleChatVoiceAndHide() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing"]
        app.launch()
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        let toggle = settings.checkBoxes["floatingOrbToggle"]
        XCTAssertTrue(toggle.exists)
        toggle.click()
        let orb = app.buttons["Open Softmax chat"]
        XCTAssertTrue(orb.waitForExistence(timeout: 5))
        print("ORB BEFORE CLICK: \(app.debugDescription)")
        orb.click()
        print("ORB AFTER CLICK: \(app.debugDescription)")
        let chat = app.windows["Softmax Conversation"]
        XCTAssertTrue(chat.waitForExistence(timeout: 5))
        XCTAssertTrue(chat.textViews["Message draft"].exists)
        XCTAssertFalse(chat.buttons["Send"].isEnabled)
        orb.rightClick()
        XCTAssertTrue(chat.staticTexts["Voice capture is not connected yet. No microphone audio is being recorded."].waitForExistence(timeout: 5))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "orb-voice-panel"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        settings.click()
        toggle.click()
        XCTAssertFalse(orb.exists)
        XCTAssertFalse(chat.exists)
        app.terminate()
    }

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
        app.terminate()
    }
}
