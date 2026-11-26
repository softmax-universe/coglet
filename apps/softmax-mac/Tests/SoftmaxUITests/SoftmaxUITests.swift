import XCTest

final class SoftmaxUITests: XCTestCase {
    // Local debug-test evidence, NOT a product wire model or an observation grant.
    private struct CatalogProbe: Decodable {
        struct Registration: Decodable {
            let id: String
            let window: String
            let route: String
            let role: String
            let verbs: [String]
            let sensitive: Bool
            let enabled: Bool
            let visible: Bool
        }
        let request: String
        let revision: UInt64
        let registrationFailed: Bool
        let registrations: [Registration]
    }

    private func catalogMembershipFailures(actual: [String], registered: Set<String>) -> [String] {
        var failures: [String] = []
        let occurrences = Dictionary(grouping: actual, by: { $0 })
        if occurrences[""] != nil { failures.append("empty-id") }
        for (id, instances) in occurrences where instances.count != 1 { failures.append("duplicate:" + id) }
        for id in Set(actual).subtracting(registered) { failures.append("unregistered:" + id) }
        for id in registered.subtracting(actual) { failures.append("missing-ax:" + id) }
        return failures.sorted()
    }

    /// Capture a fresh native presentation, never a mutation of prior evidence.
    @MainActor
    private func captureWelcomeCatalog(omitContinueRegistration: Bool = false) throws
        -> (probe: CatalogProbe, actual: [String]) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("softmax-catalog-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--catalog-probe-directory", directory.path]
        if omitContinueRegistration {
            app.launchArguments.append("--catalog-omit-welcome-continue-registration")
        }
        app.launchEnvironment["TMPDIR"] = FileManager.default.temporaryDirectory.path
        app.launch()
        defer { app.terminate() }
        let window = app.windows["Welcome to Softmax"]
        XCTAssertTrue(window.waitForExistence(timeout: 10))
        let continueButton = window.buttons["onboarding.continue"]
        XCTAssertTrue(continueButton.waitForExistence(timeout: 5))
        XCTAssertEqual(window.buttons.matching(identifier: "onboarding.continue").count, 1)
        XCTAssertEqual(continueButton.label, "Continue")
        XCTAssertTrue(continueButton.isEnabled)
        XCTAssertTrue(continueButton.isHittable)
        func captureProbe(_ name: String) throws -> CatalogProbe {
            let nonce = UUID().uuidString
            try Data(nonce.utf8).write(to: directory.appendingPathComponent("request"), options: .atomic)
            let predicate = NSPredicate { _, _ in
                guard let data = try? Data(contentsOf: directory.appendingPathComponent("response.json")),
                      let probe = try? JSONDecoder().decode(CatalogProbe.self, from: data) else { return false }
                return probe.request == nonce
            }
            let waiter = XCTNSPredicateExpectation(predicate: predicate, object: nil)
            XCTAssertEqual(XCTWaiter.wait(for: [waiter], timeout: 5), .completed)
            let bytes = try Data(contentsOf: directory.appendingPathComponent("response.json"))
            let probe = try JSONDecoder().decode(CatalogProbe.self, from: bytes)
            XCTAssertEqual(probe.request, nonce)
            XCTAssertFalse(probe.registrationFailed, "an AX id cannot hide a refused registry mount")
            XCTAssertEqual(Set(probe.registrations.map(\.id)).count, probe.registrations.count)
            let attachment = XCTAttachment(data: bytes, uniformTypeIdentifier: "public.json")
            attachment.name = name; attachment.lifetime = .keepAlways; add(attachment)
            return probe
        }
        let mode = omitContinueRegistration ? "omitted" : "normal"
        let probe = try captureProbe("onboarding-welcome-" + mode + "-registry")
        let entries = probe.registrations.filter { $0.window == "onboarding" && $0.route == "welcome" }
        let ids = Set(entries.map(\.id))
        XCTAssertFalse(ids.isEmpty)
        let chrome = ["_XCUI:CloseWindow": "onboarding.titlebar.close",
                      "_XCUI:MinimizeWindow": "onboarding.titlebar.minimize",
                      "_XCUI:ZoomWindow": "onboarding.titlebar.zoom"]
        var seen: [String] = []
        let interactive: [XCUIElement.ElementType] = [.button, .checkBox, .radioButton, .popUpButton,
            .comboBox, .textField, .secureTextField, .textView, .slider, .stepper, .link, .disclosureTriangle]
        for type in interactive {
            for element in window.descendants(matching: type).allElementsBoundByIndex {
                let id = chrome[element.identifier] ?? element.identifier
                XCTAssertFalse(id.isEmpty, "interactive AX element lacks stable id: \(element.label)")
                // Missing native membership is diagnosed by the shared comparator,
                // not an early unwrap that would bypass the negative control.
                if type == .button, let registration = entries.first(where: { $0.id == id }) {
                    XCTAssertEqual(registration.role, "button")
                    XCTAssertTrue(registration.verbs.contains("press"))
                }
                seen.append(id)
            }
        }
        XCTAssertTrue(seen.contains("onboarding.continue"), "must exercise a real product control")
        // The SAME native button works by mouse and default Return, and its
        // registration follows each mounted route rather than leaking the hook.
        window.buttons["onboarding.continue"].click()
        XCTAssertTrue(window.staticTexts["Let Softmax hear you."].waitForExistence(timeout: 5))
        let permissions = try captureProbe("onboarding-permissions-" + mode + "-registry")
        XCTAssertFalse(permissions.registrations.contains { $0.window == "onboarding" && $0.route == "welcome" })
        XCTAssertEqual(permissions.registrations.filter {
            $0.id == "onboarding.continue" && $0.route == "permissions" && $0.visible && $0.enabled
        }.count, 1)
        window.buttons["onboarding.back"].click()
        XCTAssertTrue(window.staticTexts["Speak without switching apps."].waitForExistence(timeout: 5))
        let returned = try captureProbe("onboarding-returned-" + mode + "-registry")
        XCTAssertFalse(returned.registrations.contains { $0.window == "onboarding" && $0.route == "permissions" })
        XCTAssertEqual(returned.registrations.filter { $0.id == "onboarding.continue" }.count,
            omitContinueRegistration ? 0 : 1)
        window.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(window.staticTexts["Let Softmax hear you."].waitForExistence(timeout: 5))
        return (probe, seen)
    }

    /// First real AX/live-registry join, not full all-state catalog coverage.
    @MainActor
    func testCatalogCoverageOnboardingWelcome() throws {
        let capture = try captureWelcomeCatalog()
        let entries = capture.probe.registrations.filter { $0.window == "onboarding" && $0.route == "welcome" }
        let seen = capture.actual
        let visible = Set(entries.filter(\.visible).map(\.id))
        XCTAssertEqual(catalogMembershipFailures(actual: seen, registered: visible), [],
            "visible registry entries and actual AX ids must agree, without duplicate collapse")
        // Mutation of captured evidence, NOT a claim that the native mount was
        // removed: exercise the SAME checker with a real AX id but no membership.
        let missingRegistration = visible.subtracting(["onboarding.continue"])
        XCTAssertEqual(catalogMembershipFailures(actual: seen, registered: missingRegistration),
            ["unregistered:onboarding.continue"])
        XCTAssertEqual(catalogMembershipFailures(actual: seen + ["onboarding.continue"], registered: visible),
            ["duplicate:onboarding.continue"])
    }

    /// A real omitted mount must fail the exact checker used by the positive case.
    /// The fresh app still exposes and operates the native button; only membership differs.
    @MainActor
    func testCatalogCoverageDetectsNativeOmittedRegistration() throws {
        let normal = try captureWelcomeCatalog()
        let omitted = try captureWelcomeCatalog(omitContinueRegistration: true)
        func visible(_ probe: CatalogProbe) -> Set<String> {
            Set(probe.registrations.filter {
                $0.window == "onboarding" && $0.route == "welcome" && $0.visible
            }.map(\.id))
        }
        XCTAssertEqual(normal.actual.sorted(), omitted.actual.sorted(), "omission must preserve the complete AX multiset")
        XCTAssertEqual(catalogMembershipFailures(actual: normal.actual, registered: visible(normal.probe)), [])
        XCTAssertEqual(visible(omitted.probe), visible(normal.probe).subtracting(["onboarding.continue"]))
        XCTAssertEqual(catalogMembershipFailures(actual: omitted.actual, registered: visible(omitted.probe)),
            ["unregistered:onboarding.continue"])
    }

    /// Bounded default-state census, not all presentation/state coverage. Keep
    /// the welcome omission control above unchanged and use its SAME comparator.
    @MainActor
    func testCatalogCoverageDefaultOnboardingAndSettingsRoutes() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("softmax-catalog-routes-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--catalog-probe-directory", directory.path,
            "-appearance", "Light", "-textScale", "1.0"]
        app.launchEnvironment["TMPDIR"] = FileManager.default.temporaryDirectory.path
        app.launch()
        defer { app.terminate() }
        var checkpoints: [String] = []

        func capture(window: XCUIElement, owner: String, route: String, sentinel: XCUIElement) throws {
            let checkpoint = owner + "." + route
            // A tab remains in the sidebar on every route, so use actual body
            // content instead. Readiness failures are not catalog mismatches.
            guard window.waitForExistence(timeout: 10), sentinel.waitForExistence(timeout: 5) else {
                XCTFail("native route readiness failed: " + checkpoint)
                return
            }
            let nonce = UUID().uuidString
            try Data(nonce.utf8).write(to: directory.appendingPathComponent("request"), options: .atomic)
            let ready = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                guard let bytes = try? Data(contentsOf: directory.appendingPathComponent("response.json")),
                      let probe = try? JSONDecoder().decode(CatalogProbe.self, from: bytes) else { return false }
                return probe.request == nonce
            }, object: nil)
            guard XCTWaiter.wait(for: [ready], timeout: 5) == .completed else {
                XCTFail("fresh nonce response unavailable: " + checkpoint)
                return
            }
            let bytes = try Data(contentsOf: directory.appendingPathComponent("response.json"))
            let probe = try JSONDecoder().decode(CatalogProbe.self, from: bytes)
            let raw = XCTAttachment(data: bytes, uniformTypeIdentifier: "public.json")
            raw.name = checkpoint + "-registry"; raw.lifetime = .keepAlways; add(raw)
            let chrome = ["_XCUI:CloseWindow": owner + ".titlebar.close",
                          "_XCUI:MinimizeWindow": owner + ".titlebar.minimize",
                          "_XCUI:ZoomWindow": owner + ".titlebar.zoom"]
            // Include disabled/offscreen controls, radio parents AND children.
            // Text views are conservatively included, never silently excluded
            // because they lack a registration; native classification is retained.
            let types: [XCUIElement.ElementType] = [.button, .checkBox, .radioGroup, .radioButton,
                .popUpButton, .comboBox, .textField, .secureTextField, .textView,
                .slider, .stepper, .link, .disclosureTriangle]
            var actual: [String] = []
            var records: [[String: Any]] = []
            for type in types {
                for element in window.descendants(matching: type).allElementsBoundByIndex {
                    let nativeID = element.identifier
                    let id = chrome[nativeID] ?? nativeID
                    let frame = element.frame
                    actual.append(id)
                    records.append(["type": String(describing: type), "raw_id": nativeID, "id": id,
                        "enabled": element.isEnabled, "hittable": element.isHittable,
                        "frame": [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height],
                        "classification": type == .textView ? "text-view-editability-unqualified-included" : "interactive"])
                }
            }
            let entries = probe.registrations.filter { $0.window == owner && $0.route == route }
            let failures = catalogMembershipFailures(actual: actual, registered: Set(entries.filter(\.visible).map(\.id)))
            let evidence: [String: Any] = ["checkpoint": checkpoint, "request": nonce,
                "ax": records, "membership_failures": failures]
            let audit = XCTAttachment(data: try JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys]),
                uniformTypeIdentifier: "public.json")
            audit.name = checkpoint + "-ax-membership"; audit.lifetime = .keepAlways; add(audit)
            // Evidence precedes assertions. Nonfatal failures retain later routes.
            checkpoints.append(checkpoint)
            XCTAssertEqual(probe.request, nonce, checkpoint)
            XCTAssertFalse(probe.registrationFailed, checkpoint)
            XCTAssertEqual(Set(probe.registrations.map(\.id)).count, probe.registrations.count, checkpoint)
            XCTAssertFalse(entries.isEmpty, "no actual route registrations: " + checkpoint)
            XCTAssertFalse(actual.isEmpty, "no native interactive elements: " + checkpoint)
            XCTAssertFalse(probe.registrations.contains { $0.window == owner && $0.route != route },
                "stale same-window route mount: " + checkpoint)
            XCTAssertEqual(failures, [], checkpoint)
        }

        let onboarding = app.windows["Welcome to Softmax"]
        try capture(window: onboarding, owner: "onboarding", route: "welcome",
            sentinel: onboarding.staticTexts["Speak without switching apps."])
        onboarding.buttons["onboarding.continue"].click()
        try capture(window: onboarding, owner: "onboarding", route: "permissions",
            sentinel: onboarding.staticTexts["Let Softmax hear you."])
        onboarding.buttons["onboarding.continue"].click()
        try capture(window: onboarding, owner: "onboarding", route: "hub",
            sentinel: onboarding.staticTexts["Voice not set up · return to Settings to review permissions and connection."])
        // Do not Finish, enroll, change login, or grant permissions.
        onboarding.buttons["onboarding.open_settings"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        let routes: [(String, XCUIElement)] = [
            ("general", settings.staticTexts["Softmax lives in your menu bar. Close windows without quitting."]),
            ("hotkey", settings.buttons["settings.hotkey.record"]),
            ("audio", settings.staticTexts["Default system input · voice processing and echo cancellation enabled."]),
            ("hub", settings.buttons["settings.hub.pair"]),
            ("coglets", settings.staticTexts["runtimeStatus"]),
            ("updates", settings.buttons["settings.updates.check"]),
            ("privacy", settings.buttons["settings.privacy.refresh_permissions"])]
        for (route, sentinel) in routes {
            settings.buttons["settings.tab." + route].click()
            try capture(window: settings, owner: "settings", route: route, sentinel: sentinel)
        }
        XCTAssertEqual(checkpoints, ["onboarding.welcome", "onboarding.permissions", "onboarding.hub",
            "settings.general", "settings.hotkey", "settings.audio", "settings.hub", "settings.coglets",
            "settings.updates", "settings.privacy"], "all ten route captures must execute")
    }

    @MainActor
    func testGeneralNativeRadioChoicesAndRouteRemount() throws {
        let app = XCUIApplication()
        // Do not pin mutable AppStorage keys in NSArgumentDomain: those
        // launch overrides outrank writes to the application's defaults.
        // The first real click establishes each group's starting selection.
        app.launchArguments = ["--ui-testing"]
        app.launch()
        defer { app.terminate() }
        let welcome = app.windows["Welcome to Softmax"]
        XCTAssertTrue(welcome.waitForExistence(timeout: 5))
        welcome.buttons["onboarding.open_settings"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        let groups = [("settings.general.appearance", ["system", "light", "dark"]),
                      ("settings.general.text_size", ["standard", "large", "largest"])]
        func assertSelection(_ parent: String, _ choices: [String], selected: String) {
            XCTAssertEqual(settings.radioButtons.count, 6, "Each actual radio is exactly one AX leaf; no cell duplicates")
            XCTAssertEqual(settings.radioGroups.matching(identifier: parent).count, 1)
            for choice in choices {
                let buttons = settings.radioButtons.matching(identifier: parent + "." + choice)
                XCTAssertEqual(buttons.count, 1)
                XCTAssertTrue(buttons.element.isEnabled)
                XCTAssertEqual((buttons.element.value as? NSNumber)?.intValue,
                    choice == selected ? 1 : 0, parent + "." + choice)
            }
        }
        for (parent, choices) in groups {
            for choice in choices {
                let button = settings.radioButtons[parent + "." + choice]
                XCTAssertTrue(button.waitForExistence(timeout: 5))
                button.click()
                assertSelection(parent, choices, selected: choice)
            }
        }
        // Route teardown and reconstruction must preserve the real model value
        // while returning exactly one native child per semantic identity.
        settings.buttons["settings.tab.audio"].click()
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"),
            object: settings.radioGroups["settings.general.appearance"])
        XCTAssertEqual(XCTWaiter.wait(for: [gone], timeout: 5), .completed)
        settings.buttons["settings.tab.general"].click()
        XCTAssertTrue(settings.radioGroups["settings.general.appearance"].waitForExistence(timeout: 5))
        assertSelection(groups[0].0, groups[0].1, selected: "dark")
        assertSelection(groups[1].0, groups[1].1, selected: "largest")
        settings.radioButtons["settings.general.appearance.light"].click()
        settings.radioButtons["settings.general.text_size.standard"].click()
    }

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
        app.launchArguments = ["--ui-testing", "-appearance", "Light", "-textScale", "1.0"]
        app.launch()
        defer { app.terminate() }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["Open Settings…"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        settings.typeText("/")
        settings.typeText("o") // Multiple matches including General, Hotkey and Audio.
        XCTAssertTrue(settings.buttons["General"].exists)
        XCTAssertTrue(settings.buttons["Hotkey"].exists)
        XCTAssertTrue(settings.buttons["Audio"].exists)
        settings.typeKey(.downArrow, modifierFlags: [])
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Focused search Down must select Hotkey")
        settings.typeKey(.downArrow, modifierFlags: [])
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.checkBoxes["Play sound cues"].exists, "Second Down must select Audio")
        settings.typeKey(.upArrow, modifierFlags: [])
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Up must return to Hotkey")
        settings.typeKey("a", modifierFlags: .command)
        settings.typeText("zz-no-matches")
        XCTAssertTrue(settings.staticTexts["No matching settings"].exists)
        settings.typeKey(.downArrow, modifierFlags: [])
        settings.typeKey(.upArrow, modifierFlags: [])
        settings.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Empty search must retain the current section")
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
