import XCTest

final class SoftmaxUITests: XCTestCase {
    /// Native CI on macOS 26 reports two OS-owned chrome defects:
    /// its disabled empty application Touch Bar, and the Zoom button's internal
    /// disabled 14 pt group. Never exempt a product control or any contrast issue.
    @MainActor
    private func isSystemChrome(_ issue: XCUIAccessibilityAuditIssue) -> Bool {
        guard let element = issue.element, !element.isEnabled else { return false }
        if issue.auditType == .sufficientElementDescription,
           element.elementType == .touchBar && element.children(matching: .any).count == 0 {
            return true
        }
        return issue.auditType == .parentChild && element.elementType == .group
            && element.frame.size == CGSize(width: 14, height: 14)
            && element.debugDescription.contains("identifier: '_XCUI:ZoomWindow'")
    }

    @MainActor
    func testOnboardingAndEverySettingsSectionAccessibilityAudit() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-appearance", "System"]
        app.launch()
        defer { app.terminate() }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        for step in 0..<3 {
            let beforeAudit = XCTAttachment(screenshot: onboarding.screenshot())
            beforeAudit.name = "before-audit-onboarding-\(step)"; beforeAudit.lifetime = .keepAlways; add(beforeAudit)
            print("AX onboarding chapter \(step):\n\(app.debugDescription)")
            try app.performAccessibilityAudit { issue in
                print("AX audit issue: \(issue.detailedDescription); element: \(String(describing: issue.element))")
                return self.isSystemChrome(issue)
            }
            let image = XCTAttachment(screenshot: onboarding.screenshot())
            image.name = "onboarding-step-\(step)"; image.lifetime = .keepAlways; add(image)
            if step < 2 { onboarding.buttons["Continue"].click() }
        }
        onboarding.buttons["Back"].click()
        XCTAssertTrue(onboarding.buttons["Request Input Monitoring"].exists)
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        for section in ["General", "Hotkey", "Audio", "Hub", "Coglets", "Updates", "Privacy"] {
            XCTAssertFalse(onboarding.exists, "Settings must not occlude unfinished onboarding")
            settings.buttons[section].click()
            let beforeAudit = XCTAttachment(screenshot: settings.screenshot())
            beforeAudit.name = "before-audit-settings-\(section)"; beforeAudit.lifetime = .keepAlways; add(beforeAudit)
            print("AX settings \(section):\n\(app.debugDescription)")
            try app.performAccessibilityAudit { issue in
                print("AX audit issue: \(issue.detailedDescription); element: \(String(describing: issue.element))")
                return self.isSystemChrome(issue)
            }
            let image = XCTAttachment(screenshot: settings.screenshot())
            image.name = "settings-\(section)"; image.lifetime = .keepAlways; add(image)
        }
        settings.typeKey("k", modifierFlags: .command)
        settings.typeText("microphone")
        XCTAssertTrue(settings.buttons["Audio"].exists)
        XCTAssertFalse(settings.buttons["General"].exists)
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.buttons["Microphone Settings"].exists)
        settings.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(settings.buttons["General"].exists)
    }

    @MainActor
    func testContrastCalibration() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--contrast-calibration"]
        app.launch()
        defer { app.terminate() }
        XCTAssertTrue(app.windows["Contrast calibration"].waitForExistence(timeout: 10))
        let image = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        image.name = "contrast-calibration"; image.lifetime = .keepAlways; add(image)
        try app.performAccessibilityAudit(for: .contrast) { issue in
            print("CALIBRATION: \(issue.detailedDescription); identifier=\(issue.element?.identifier ?? "nil"); frame=\(String(describing: issue.element?.frame))")
            return false
        }
    }

    @MainActor
    func testFacePanelExpandDemoAndEscape() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--show-face"]
        app.launch()
        let face = app.dialogs["Softmax Face"]
        XCTAssertTrue(face.waitForExistence(timeout: 10))
        XCTAssertTrue(face.buttons["Compact Face"].exists)
        face.buttons["Compact Face"].click()
        XCTAssertTrue(face.buttons["Expand Face"].exists)
        face.buttons["Expand Face"].click()
        face.buttons["Start simulated demo"].click()
        XCTAssertTrue(face.staticTexts["Simulated demo · no audio"].exists)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "native-face-panel"; screenshot.lifetime = .keepAlways; add(screenshot)
        face.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(face.exists)
        app.terminate()
    }

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
        orb.click()
        // AppKit exposes NSPanel as AXDialog, not AXWindow.
        let chat = app.dialogs["Softmax Conversation"]
        XCTAssertTrue(chat.waitForExistence(timeout: 5))
        XCTAssertTrue(chat.textViews["Message draft"].exists)
        XCTAssertFalse(chat.buttons["Send"].isEnabled)
        orb.rightClick()
        XCTAssertTrue(chat.staticTexts["Use the push-to-talk hotkey to capture audio. Runtime speech delivery is not connected yet."].waitForExistence(timeout: 5))
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
