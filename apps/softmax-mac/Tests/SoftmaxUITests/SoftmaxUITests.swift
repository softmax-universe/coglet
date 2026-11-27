import AppKit
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
        try auditOnboardingAndSettings(appearance: "Light")
        try auditOnboardingAndSettings(appearance: "Dark")
    }

    @MainActor
    private func auditOnboardingAndSettings(appearance: String) throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-appearance", appearance, "-textScale", "1.0"]
        app.launch()
        defer { app.terminate() }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        for step in 0..<3 {
            let beforeAudit = XCTAttachment(screenshot: onboarding.screenshot())
            beforeAudit.name = "\(appearance)-before-audit-onboarding-\(step)"; beforeAudit.lifetime = .keepAlways; add(beforeAudit)
            print("AX onboarding chapter \(step):\n\(app.debugDescription)")
            try app.performAccessibilityAudit { issue in
                print("AX audit issue: \(issue.detailedDescription); element: \(String(describing: issue.element))")
                return self.isSystemChrome(issue)
            }
            let image = XCTAttachment(screenshot: onboarding.screenshot())
            image.name = "\(appearance)-onboarding-step-\(step)"; image.lifetime = .keepAlways; add(image)
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
            beforeAudit.name = "\(appearance)-before-audit-settings-\(section)"; beforeAudit.lifetime = .keepAlways; add(beforeAudit)
            print("AX settings \(section):\n\(app.debugDescription)")
            try app.performAccessibilityAudit { issue in
                print("AX audit issue: \(issue.detailedDescription); element: \(String(describing: issue.element))")
                return self.isSystemChrome(issue)
            }
            let image = XCTAttachment(screenshot: settings.screenshot())
            image.name = "\(appearance)-settings-\(section)"; image.lifetime = .keepAlways; add(image)
        }
        settings.typeText("/")
        settings.typeText("microphone")
        XCTAssertTrue(settings.buttons["Audio"].exists)
        XCTAssertFalse(settings.buttons["General"].exists)
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.buttons["Microphone Settings"].exists)
        settings.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(settings.buttons["General"].exists)
    }

    @MainActor
    private func reveal(_ element: XCUIElement, in scroll: XCUIElement) {
        for _ in 0..<8 {
            if scroll.frame.contains(element.frame) { break }
            let old = element.frame
            scroll.scroll(byDeltaX: 0, deltaY: element.frame.minY < scroll.frame.minY ? 200 : -200)
            if element.frame == old { break }
        }
        XCTAssertTrue(scroll.frame.contains(element.frame), "Text/control must be fully reachable, not clipped: \(element)")
        XCTAssertTrue(element.isHittable)
    }

    @MainActor
    func testLargestTextScrolling() {
        for appearance in ["Light", "Dark"] {
            let app = XCUIApplication()
            app.launchArguments = ["--ui-testing", "-appearance", appearance, "-textScale", "1.6"]
            app.launch()
            let onboarding = app.windows["Welcome to Softmax"]
            XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
            onboarding.buttons["Continue"].click()
            let scroll = onboarding.scrollViews.firstMatch
            reveal(onboarding.buttons["Request Input Monitoring"], in: scroll)
            let image = XCTAttachment(screenshot: onboarding.screenshot())
            image.name = "\(appearance)-largest-voice-scrolled"; image.lifetime = .keepAlways; add(image)
            onboarding.buttons["Back"].click()
            onboarding.buttons["Continue"].click()
            XCTAssertTrue(onboarding.staticTexts["Let Softmax hear you."].exists)
            onboarding.buttons["Open Settings…"].click()
            let settings = app.windows["Softmax Settings"]
            XCTAssertTrue(settings.waitForExistence(timeout: 5))
            reveal(settings.buttons["Login Item Settings"], in: settings.scrollViews.firstMatch)
            let general = XCTAttachment(screenshot: settings.screenshot())
            general.name = "\(appearance)-largest-general-scrolled"; general.lifetime = .keepAlways; add(general)
            app.terminate()
        }
    }

    @MainActor
    func testGlobalCommandsAndFocusReturn() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", "1.0"]
        app.launch()
        defer { app.terminate() }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.typeKey("k", modifierFlags: .command)
        let palette = app.descendants(matching: .any).matching(identifier: "Softmax Commands").firstMatch
        XCTAssertTrue(palette.waitForExistence(timeout: 5))
        palette.typeText("privacy")
        XCTAssertTrue(palette.buttons["Settings → Privacy"].exists)
        XCTAssertFalse(palette.buttons["Settings → General"].exists)
        palette.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(palette.exists)
        onboarding.typeKey("k", modifierFlags: .command)
        XCTAssertTrue(palette.waitForExistence(timeout: 5))
        palette.typeKey(.downArrow, modifierFlags: [])
        palette.typeKey(.return, modifierFlags: [])
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists)
        settings.typeKey("k", modifierFlags: .command)
        palette.typeText("version")
        XCTAssertTrue(palette.buttons["Settings → Updates"].exists)
        XCTAssertTrue(palette.buttons["About Softmax"].exists)
        palette.typeKey(.downArrow, modifierFlags: [])
        palette.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.windows["About Softmax"].waitForExistence(timeout: 5))
        app.windows["About Softmax"].typeKey("k", modifierFlags: .command)
        palette.typeKey(.escape, modifierFlags: [])
        XCTAssertFalse(palette.exists)
        XCTAssertTrue(app.windows["About Softmax"].exists)
    }

    @MainActor
    func testCommandSelectionStaysVisible() {
        for scale in ["1.0", "1.6"] {
            let app = XCUIApplication()
            app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", scale]
            app.launch()
            let onboarding = app.windows["Welcome to Softmax"]
            XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
            onboarding.typeKey("k", modifierFlags: .command)
            let palette = app.descendants(matching: .any).matching(identifier: "Softmax Commands").firstMatch
            XCTAssertTrue(palette.waitForExistence(timeout: 5))
            let scroll = palette.scrollViews.firstMatch
            // AX includes the native bezel's 1pt outsets above and below the
            // explicit SwiftUI layout frame (25/35pt); assert both scale anchors.
            let expectedSearchHeight: CGFloat = scale == "1.0" ? 27 : 37
            let search = palette.textFields["Search commands and settings"]
            XCTAssertTrue(search.exists)
            XCTAssertEqual(search.frame.height, expectedSearchHeight, accuracy: 0.5,
                           "Command search must use the reviewed scale-specific height")
            // Keyboard input only: clicking a row would auto-scroll and hide the regression.
            for _ in 0..<10 { palette.typeKey(.downArrow, modifierFlags: []) }
            let last = palette.buttons["About Softmax"]
            XCTAssertTrue(scroll.frame.contains(last.frame), "Last keyboard selection must be fully visible at scale \(scale)")
            XCTAssertTrue(last.isHittable)
            let image = XCTAttachment(screenshot: palette.screenshot())
            image.name = "commands-last-selected-\(scale)"; image.lifetime = .keepAlways; add(image)
            palette.typeKey(.upArrow, modifierFlags: [])
            XCTAssertTrue(scroll.frame.contains(palette.buttons["Open Voice"].frame))
            palette.typeKey(.downArrow, modifierFlags: [])
            palette.typeKey(.return, modifierFlags: [])
            let about = app.windows["About Softmax"]
            XCTAssertTrue(about.waitForExistence(timeout: 5), "Return must open the selected last command")
            about.typeKey("k", modifierFlags: .command)
            XCTAssertTrue(palette.waitForExistence(timeout: 5))
            for _ in 0..<10 { palette.typeKey(.downArrow, modifierFlags: []) }
            palette.typeText("privacy")
            let filtered = palette.buttons["Settings → Privacy"]
            XCTAssertTrue(filtered.exists)
            XCTAssertTrue(scroll.frame.contains(filtered.frame), "Filtering from the bottom must reveal the first match")
            palette.typeKey(.return, modifierFlags: [])
            XCTAssertTrue(app.windows["Softmax Settings"].buttons["Refresh Permissions"].waitForExistence(timeout: 5))
            let settingsSearch = app.windows["Softmax Settings"].textFields["Search settings"]
            XCTAssertTrue(settingsSearch.exists)
            XCTAssertEqual(settingsSearch.frame.height, expectedSearchHeight, accuracy: 0.5,
                           "Settings filter must share command search geometry")
            app.terminate()
        }
    }

    @MainActor
    func testSettingsFocusedSearchArrowNavigation() {
        let app = XCUIApplication()
        let probe = FileManager.default.temporaryDirectory.appendingPathComponent("settings-arrow-\(UUID().uuidString).json")
        app.launchEnvironment["SOFTMAX_SETTINGS_ARROW_PROBE"] = probe.path
        var driver: [[String: Any]] = []
        driver.reserveCapacity(32)
        func input(_ operation: String, _ body: () -> Void) {
            func mark(_ edge: String) {
                guard driver.count < 32 else { XCTFail("Driver capture overflow"); return }
                driver.append(["sequence": driver.count, "uptime": ProcessInfo.processInfo.systemUptime,
                               "operation": operation, "edge": edge])
            }
            mark("begin")
            body()
            mark("end")
        }
        defer {
            defer { app.terminate(); try? FileManager.default.removeItem(at: probe) }
            let data = try? Data(contentsOf: probe)
            let document = data.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            let records = document?["records"] as? [[String: Any]] ?? []
            let valid = document?["schema"] as? Int == 1
                && document?["boundary"] as? String == "finalEscapeBeforeDispatch"
                && document?["complete"] as? Bool == true
                && records.last?["phase"] as? String == "arrival"
                && records.last?["keyCode"] as? Int == 53
                && records.contains { $0["phase"] as? String == "arrival" }
                && driver.count == 28
            let dropped = document?["dropped"] as? Int
            let status = (dropped ?? 0) > 0 ? "overflow" : (valid && dropped == 0 ? "complete" : "missingOrInvalid")
            var envelope: [String: Any] = ["schema": 1, "captureStatus": status, "driver": driver]
            if let document { envelope["app"] = document }
            if let evidence = try? JSONSerialization.data(withJSONObject: envelope, options: [.sortedKeys]) {
                let attachment = XCTAttachment(data: evidence, uniformTypeIdentifier: "public.json")
                attachment.name = "settings-arrow-boundaries.json"
                attachment.lifetime = .keepAlways
                add(attachment)
            } else { XCTFail("Unable to encode diagnostic evidence") }
            XCTAssertEqual(status, "complete", "Incomplete boundary capture is inconclusive, not non-delivery proof")
        }
        app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", "1.0"]
        app.launch()
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        input("slash") { settings.typeText("/") }
        input("filter") { settings.typeText("o") } // Multiple matches including General, Hotkey and Audio.
        XCTAssertTrue(settings.buttons["General"].exists)
        XCTAssertTrue(settings.buttons["Hotkey"].exists)
        XCTAssertTrue(settings.buttons["Audio"].exists)
        input("firstDown") { settings.typeKey(.downArrow, modifierFlags: []) }
        input("firstReturn") { settings.typeKey(.return, modifierFlags: []) }
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Focused search Down must select Hotkey")
        input("secondDown") { settings.typeKey(.downArrow, modifierFlags: []) }
        input("secondReturn") { settings.typeKey(.return, modifierFlags: []) }
        XCTAssertTrue(settings.checkBoxes["Play sound cues"].exists, "Second Down must select Audio")
        input("up") { settings.typeKey(.upArrow, modifierFlags: []) }
        input("upReturn") { settings.typeKey(.return, modifierFlags: []) }
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Up must return to Hotkey")
        input("selectAll") { settings.typeKey("a", modifierFlags: .command) }
        input("noMatches") { settings.typeText("zz-no-matches") }
        XCTAssertTrue(settings.staticTexts["No matching settings"].exists)
        input("emptyDown") { settings.typeKey(.downArrow, modifierFlags: []) }
        input("emptyUp") { settings.typeKey(.upArrow, modifierFlags: []) }
        input("emptyReturn") { settings.typeKey(.return, modifierFlags: []) }
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Empty search must retain the current section")
        input("escape") { settings.typeKey(.escape, modifierFlags: []) }
        XCTAssertTrue(settings.buttons["General"].exists)
    }

    /// Matched enabled/default experiment for the AutoFill helper that appeared
    /// before every lost first Down in native failures. Phase E (argument domain
    /// re-enables AutoFill heuristics) qualifies the detector and records the
    /// same boundaries as Phase D, which must never start the helper and must
    /// navigate on immediate input. Mechanism evidence only; not a race reproducer.
    @MainActor
    func testSettingsSearchDoesNotStartAutoFillHelper() {
        let helperID = "com.apple.SafariPlatformSupport.Helper"
        func current() -> Set<Int32> {
            Set(NSRunningApplication.runningApplications(withBundleIdentifier: helperID).map(\.processIdentifier))
        }
        func waitUntil(_ seconds: TimeInterval, _ condition: () -> Bool) -> Bool {
            let deadline = ProcessInfo.processInfo.systemUptime + seconds
            while ProcessInfo.processInfo.systemUptime < deadline {
                if condition() { return true }
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
            }
            return condition()
        }
        var evidence: [[String: Any]] = []
        defer {
            if let data = try? JSONSerialization.data(withJSONObject: ["schema": 1, "phases": evidence], options: [.sortedKeys]) {
                let attachment = XCTAttachment(data: data, uniformTypeIdentifier: "public.json")
                attachment.name = "settings-autofill-helper.json"; attachment.lifetime = .keepAlways; add(attachment)
            } else { XCTFail("Unable to encode AutoFill helper evidence") }
        }
        // Returns new helper PIDs (relative to the pre-launch baseline) and the
        // selection result after the original immediate Down/Return sequence.
        func phase(_ name: String, enableAutoFill: Bool) -> (helpers: Set<Int32>, hotkey: Bool) {
            let baseline = current()
            let sampler = AutoFillHelperSampler(bundleID: helperID)
            let app = XCUIApplication()
            app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", "1.0"]
                + (enableAutoFill ? ["-NSAutoFillHeuristicsEnabled", "YES"] : [])
            app.launch()
            var record: [String: Any] = ["phase": name, "baseline": baseline.sorted()]
            let onboarding = app.windows["Welcome to Softmax"]
            XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
            onboarding.buttons["Open Settings…"].click()
            let settings = app.windows["Softmax Settings"]
            XCTAssertTrue(settings.waitForExistence(timeout: 5))
            let field = settings.textFields["Search settings"]
            settings.typeText("/")
            settings.typeText("o")
            let filterEnd = ProcessInfo.processInfo.systemUptime
            if enableAutoFill {
                // Positive control: aim Down at the helper start window seen in failures.
                let seen = waitUntil(5) { !current().subtracting(baseline).isEmpty }
                record["positiveControlSeen"] = seen
                XCTAssertTrue(seen, "Detector unqualified: re-enabled AutoFill never showed its helper")
            }
            record["focusBeforeDown"] = (field.value(forKey: "hasKeyboardFocus") as? Bool).map { $0 as Any } ?? NSNull()
            record["settingsExistsBeforeDown"] = settings.exists
            let downBegin = ProcessInfo.processInfo.systemUptime
            settings.typeKey(.downArrow, modifierFlags: [])
            let downEnd = ProcessInfo.processInfo.systemUptime
            settings.typeKey(.return, modifierFlags: [])
            let hotkey = settings.buttons["Record Hotkey…"].exists
            record["filterToDown"] = downBegin - filterEnd
            record["downDuration"] = downEnd - downBegin
            record["hotkeySelected"] = hotkey
            // Equal 5 s observation window from the end of filter input.
            _ = waitUntil(max(0, filterEnd + 5 - ProcessInfo.processInfo.systemUptime)) { false }
            app.terminate()
            let (samples, overflow) = sampler.stop()
            let started = samples.reduce(into: Set<Int32>()) { $0.formUnion($1.pids) }.subtracting(baseline)
            let running = NSRunningApplication.runningApplications(withBundleIdentifier: helperID)
            record["newHelpers"] = started.sorted().map { pid -> [String: Any] in
                let launch = running.first { $0.processIdentifier == pid }?.launchDate?.timeIntervalSince1970
                return ["pid": pid, "launchDate": launch.map { $0 as Any } ?? NSNull(),
                        "firstSeenAfterFilter": (samples.first { $0.pids.contains(pid) }?.uptime ?? filterEnd) - filterEnd]
            }
            // Attribution: helpers started for this app exit after it terminates.
            let exited = waitUntil(10) { current().isDisjoint(with: started) }
            record["newHelpersExitedAfterTerminate"] = exited
            record["samples"] = samples.count
            record["samplerOverflow"] = overflow
            XCTAssertFalse(overflow, "Helper sampler overflow is inconclusive")
            if enableAutoFill { XCTAssertTrue(exited, "Positive-control helper not attributable to the app") }
            evidence.append(record)
            return (started, hotkey)
        }
        let enabled = phase("autoFillEnabled", enableAutoFill: true)
        XCTAssertFalse(enabled.helpers.isEmpty, "Detector unqualified: no helper sampled with AutoFill enabled")
        let product = phase("productDefault", enableAutoFill: false)
        XCTAssertTrue(product.helpers.isEmpty, "Settings search must not start the AutoFill helper")
        XCTAssertTrue(product.hotkey, "Immediate focused search Down must select Hotkey")
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

    /// The shared configuration entry must exist in both host surfaces without
    /// changing Settings navigation or hiding onboarding's login/voice controls.
    @MainActor
    func testRouterEnrollmentConfigurationInOnboardingAndSettings() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", "1.0"]
        app.launch()
        defer { app.terminate() }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["Continue"].click()
        onboarding.buttons["Continue"].click()
        let pair = onboarding.buttons["Pair router…"]
        guard pair.waitForExistence(timeout: 5) else {
            XCTFail("Final onboarding must expose router enrollment"); return
        }
        XCTAssertTrue(onboarding.buttons["Login Item Settings"].exists)
        XCTAssertTrue(onboarding.buttons["Finish"].exists)
        pair.click()
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        XCTAssertTrue(sheet.secureTextFields["Pairing code"].exists)
        XCTAssertTrue(sheet.buttons["Import QR image…"].exists)
        XCTAssertFalse(sheet.buttons["Connect router"].isEnabled, "An empty code must never enroll")
        // AutoFill heuristics are off; the secure pairing field must still accept typed input.
        let code = sheet.secureTextFields["Pairing code"]
        code.click()
        code.typeText("abc")
        XCTAssertFalse((code.value as? String ?? "").isEmpty, "Pairing code must accept typed input")
        sheet.buttons["Cancel"].click()
        XCTAssertFalse(sheet.exists)
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        settings.buttons["Hub"].click()
        XCTAssertTrue(settings.textFields["Search settings"].exists)
        XCTAssertTrue(settings.buttons["Keyboard shortcuts  ?"].exists)
        XCTAssertTrue(settings.buttons["Pair router…"].exists)
        settings.buttons["Pair router…"].click()
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        XCTAssertTrue(sheet.secureTextFields["Pairing code"].exists)
        XCTAssertTrue(sheet.buttons["Import QR image…"].exists)
        sheet.buttons["Cancel"].click()
        XCTAssertFalse(sheet.exists)
        XCTAssertTrue(settings.buttons["General"].exists)
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

/// Samples AutoFill helper PIDs off the XCUI input path (20 ms, bounded).
private final class AutoFillHelperSampler: @unchecked Sendable {
    private let lock = NSLock()
    private var samples: [(uptime: TimeInterval, pids: [Int32])] = []
    private var overflow = false
    private let timer = DispatchSource.makeTimerSource(queue: .global(qos: .userInitiated))

    init(bundleID: String) {
        samples.reserveCapacity(1024)
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            let pids = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
                .map(\.processIdentifier).sorted()
            self?.append(ProcessInfo.processInfo.systemUptime, pids)
        }
        timer.resume()
    }

    private func append(_ uptime: TimeInterval, _ pids: [Int32]) {
        lock.lock(); defer { lock.unlock() }
        if samples.count < 1024 { samples.append((uptime, pids)) } else { overflow = true }
    }

    func stop() -> (samples: [(uptime: TimeInterval, pids: [Int32])], overflow: Bool) {
        timer.cancel()
        lock.lock(); defer { lock.unlock() }
        return (samples, overflow)
    }
}
