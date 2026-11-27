import XCTest

/// Gate-1 (`softmax_app_catalog_coverage`) breadth extension beyond the ten
/// default onboarding/settings routes in `SoftmaxUITests`. This is NOT full
/// ADR-0030 coverage and NOT all fourteen windows: `testCoverageLedger…` pins
/// exactly which windows are covered here and which remain unresolved (and why).
///
/// Same method as the baseline census, with nothing relaxed:
///  - the SAME comparator text (byte-parity asserted against SoftmaxUITests.swift),
///  - an independent actual XCUI tree enumeration of each real native surface,
///  - a fresh-nonce registry probe written/read through the existing
///    `--catalog-probe-directory` seam (no new launch flag or harness),
///  - every state is reached by an ordinary native user action (click, key,
///    right-click). Nothing is granted, typed into a secret field, enrolled,
///    imported, opened in System Settings or sent to a URL.
/// Evidence attachments precede assertions so one red state keeps later states.
final class SoftmaxCatalogCoverageExtensionTests: XCTestCase {
    // Local debug-test evidence only, identical in shape to the baseline probe.
    private struct CoverageProbe: Decodable {
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

    private struct CoverageResult {
        let probe: CoverageProbe
        let entries: [CoverageProbe.Registration]
        let actual: [String]
        let failures: [String]
    }

    /// VERBATIM copy of SoftmaxUITests.catalogMembershipFailures (that method is
    /// private). `testCoverageComparatorIsIdenticalToBaseline` fails if either
    /// text drifts, so this file can never silently weaken or fork the checker.
    private func catalogMembershipFailures(actual: [String], registered: Set<String>) -> [String] {
        var failures: [String] = []
        let occurrences = Dictionary(grouping: actual, by: { $0 })
        if occurrences[""] != nil { failures.append("empty-id") }
        for (id, instances) in occurrences where instances.count != 1 { failures.append("duplicate:" + id) }
        for id in Set(actual).subtracting(registered) { failures.append("unregistered:" + id) }
        for id in registered.subtracting(actual) { failures.append("missing-ax:" + id) }
        return failures.sorted()
    }

    /// The baseline census element types, plus `.menuButton` (strictly additive:
    /// SwiftUI `Menu` may surface as a menu button rather than a pop-up button).
    private let coverageTypes: [XCUIElement.ElementType] = [.button, .checkBox, .radioGroup, .radioButton,
        .popUpButton, .comboBox, .textField, .secureTextField, .textView, .slider, .stepper, .link,
        .disclosureTriangle, .menuButton]

    private var probeDirectory: URL?

    // MARK: - Harness (reuses the existing probe seam; no new launch flags)

    @MainActor
    private func launchProbed(_ extra: [String] = []) throws -> XCUIApplication {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("softmax-catalog-ext-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        probeDirectory = directory
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--catalog-probe-directory", directory.path,
            "-appearance", "Light", "-textScale", "1.0"] + extra
        app.launchEnvironment["TMPDIR"] = FileManager.default.temporaryDirectory.path
        app.launch()
        return app
    }

    @MainActor
    private func finish(_ app: XCUIApplication) {
        app.terminate()
        if let directory = probeDirectory { try? FileManager.default.removeItem(at: directory) }
        probeDirectory = nil
    }

    @MainActor
    private func waitUntil(_ timeout: TimeInterval = 5, _ predicate: @escaping @MainActor () -> Bool) -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .milliseconds(Int64(timeout * 1_000)))
        while clock.now < deadline {
            if predicate() { return true }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
        }
        return false
    }

    /// Fresh nonce, fresh response, same wait as the baseline census.
    @MainActor
    private func freshProbe(_ checkpoint: String) throws -> (probe: CoverageProbe, bytes: Data, nonce: String)? {
        guard let directory = probeDirectory else { XCTFail("no probe directory: " + checkpoint); return nil }
        let nonce = UUID().uuidString
        try Data(nonce.utf8).write(to: directory.appendingPathComponent("request"), options: .atomic)
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard let bytes = try? Data(contentsOf: directory.appendingPathComponent("response.json")),
                  let probe = try? JSONDecoder().decode(CoverageProbe.self, from: bytes) else { return false }
            return probe.request == nonce
        }, object: nil)
        guard XCTWaiter.wait(for: [ready], timeout: 5) == .completed else {
            XCTFail("fresh nonce response unavailable: " + checkpoint)
            return nil
        }
        let bytes = try Data(contentsOf: directory.appendingPathComponent("response.json"))
        return (try JSONDecoder().decode(CoverageProbe.self, from: bytes), bytes, nonce)
    }

    /// One checkpoint: independent actual AX ids of `scope` vs the live registry's
    /// visible entries for exactly (owner, route), through the shared comparator.
    /// `anchors` must be present in BOTH sides (so the state cannot pass vacuously);
    /// `absent` must be present in NEITHER (a retired dynamic mount).
    @MainActor
    @discardableResult
    private func captureCoverage(scope: XCUIElement, owner: String, route: String, tag: String,
        sentinel: XCUIElement, anchors: [String], absent: [String] = [],
        expecting: [String: (role: String, verbs: [String], sensitive: Bool)] = [:],
        types: [XCUIElement.ElementType]? = nil, restrictRegisteredTo: Set<String>? = nil) throws -> CoverageResult? {
        let checkpoint = owner + "." + route + "." + tag
        guard scope.waitForExistence(timeout: 10), sentinel.waitForExistence(timeout: 5) else {
            XCTFail("native state readiness failed: " + checkpoint)
            return nil
        }
        guard let fresh = try freshProbe(checkpoint) else { return nil }
        let probe = fresh.probe
        let raw = XCTAttachment(data: fresh.bytes, uniformTypeIdentifier: "public.json")
        raw.name = checkpoint + "-registry"; raw.lifetime = .keepAlways; add(raw)
        let chrome = ["_XCUI:CloseWindow": owner + ".titlebar.close",
                      "_XCUI:MinimizeWindow": owner + ".titlebar.minimize",
                      "_XCUI:ZoomWindow": owner + ".titlebar.zoom"]
        var actual: [String] = []
        var records: [[String: Any]] = []
        for type in types ?? coverageTypes {
            for element in scope.descendants(matching: type).allElementsBoundByIndex {
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
        var visible = Set(entries.filter(\.visible).map(\.id))
        if let restrict = restrictRegisteredTo { visible = visible.intersection(restrict) }
        let failures = catalogMembershipFailures(actual: actual, registered: visible)
        var evidence: [String: Any] = ["checkpoint": checkpoint, "request": fresh.nonce, "ax": records,
            "membership_failures": failures, "anchors": anchors, "absent": absent]
        // Removable run49 measurement only: never feeds actual/visible or the comparator.
        // Scope-local snapshots use untouched UI-test defaults; omit field values.
        if ["command_palette", "hub_pairing", "chat"].contains(owner) {
            let debugLimit = 24_000, scrollBarLimit = 8, buttonLimit = 32
            let description = scope.debugDescription.replacingOccurrences(
                of: "(?im)\\bvalue:.*$", with: "value: <omitted>", options: .regularExpression)
            evidence["ax_debug_description"] = String(description.prefix(debugLimit))
            evidence["ax_debug_description_truncated"] = description.count > debugLimit
            let scrollBars = scope.descendants(matching: .scrollBar)
            var scrollBarRecords: [[String: Any]] = []
            for (parentIndex, scrollBar) in scrollBars.allElementsBoundByIndex.prefix(scrollBarLimit).enumerated() {
                let parentFrame = scrollBar.frame
                // Independent query under this specific scrollBar, NOT a geometry-based inference
                // from the census. Empty results and truncation remain explicit evidence.
                let buttons = scrollBar.descendants(matching: .button)
                var buttonRecords: [[String: Any]] = []
                for (descendantIndex, button) in buttons.allElementsBoundByIndex.prefix(buttonLimit).enumerated() {
                    let frame = button.frame
                    buttonRecords.append(["parent_scrollbar_index": parentIndex,
                        "descendant_index": descendantIndex, "type": String(describing: button.elementType),
                        "raw_id": button.identifier,
                        "frame": [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]])
                }
                scrollBarRecords.append(["scrollbar_index": parentIndex,
                    "type": String(describing: scrollBar.elementType), "raw_id": scrollBar.identifier,
                    "frame": [parentFrame.origin.x, parentFrame.origin.y, parentFrame.size.width, parentFrame.size.height],
                    "descendant_button_count": buttons.count, "descendant_buttons": buttonRecords,
                    "descendant_buttons_truncated": buttons.count > buttonLimit])
            }
            evidence["scrollbar_count"] = scrollBars.count
            evidence["scrollbars_truncated"] = scrollBars.count > scrollBarLimit
            evidence["scrollbar_descendants"] = scrollBarRecords
        }
        let audit = XCTAttachment(data: try JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys]),
            uniformTypeIdentifier: "public.json")
        audit.name = checkpoint + "-ax-membership"; audit.lifetime = .keepAlways; add(audit)
        // Evidence precedes assertions; every assertion is non-fatal.
        XCTAssertEqual(probe.request, fresh.nonce, checkpoint)
        XCTAssertFalse(probe.registrationFailed, checkpoint)
        XCTAssertEqual(Set(probe.registrations.map(\.id)).count, probe.registrations.count, checkpoint)
        XCTAssertFalse(entries.isEmpty, "no actual route registrations: " + checkpoint)
        XCTAssertFalse(actual.isEmpty, "no native interactive elements: " + checkpoint)
        XCTAssertFalse(probe.registrations.contains { $0.window == owner && $0.route != route },
            "stale same-window route mount: " + checkpoint)
        for anchor in anchors {
            XCTAssertTrue(actual.contains(anchor), "anchor missing from actual AX tree: " + anchor + " @ " + checkpoint)
            XCTAssertTrue(visible.contains(anchor), "anchor missing from visible registry: " + anchor + " @ " + checkpoint)
        }
        for id in absent {
            XCTAssertFalse(actual.contains(id), "retired control still in AX tree: " + id + " @ " + checkpoint)
            XCTAssertFalse(visible.contains(id), "retired control still visible in registry: " + id + " @ " + checkpoint)
        }
        for (id, wanted) in expecting {
            guard let entry = entries.first(where: { $0.id == id }) else {
                XCTFail("no registration for expected control: " + id + " @ " + checkpoint); continue
            }
            XCTAssertEqual(entry.role, wanted.role, id + " @ " + checkpoint)
            XCTAssertEqual(entry.verbs.sorted(), wanted.verbs.sorted(), id + " @ " + checkpoint)
            XCTAssertEqual(entry.sensitive, wanted.sensitive, id + " @ " + checkpoint)
        }
        XCTAssertEqual(failures, [], checkpoint)
        return CoverageResult(probe: probe, entries: entries, actual: actual, failures: failures)
    }

    /// A closed/retired presentation must leave no visible registration behind.
    @MainActor
    private func assertNoVisibleRegistrations(window: String, tag: String) throws {
        guard let fresh = try freshProbe(window + ".closed." + tag) else { return }
        let raw = XCTAttachment(data: fresh.bytes, uniformTypeIdentifier: "public.json")
        raw.name = window + ".closed." + tag + "-registry"; raw.lifetime = .keepAlways; add(raw)
        XCTAssertFalse(fresh.probe.registrationFailed, window + " closed " + tag)
        XCTAssertEqual(fresh.probe.registrations.filter { $0.window == window && $0.visible }.map(\.id), [],
            "closed presentation must not keep visible registrations: " + window + " " + tag)
    }

    @MainActor
    private static func disclosureValue(_ element: XCUIElement) -> Int? {
        if let text = element.value as? String { return Int(text) }
        return (element.value as? NSNumber)?.intValue
    }

    /// Real native user click on the chevron of an identified disclosure triangle
    /// (the AX frame includes its label, so click 26 points in, as the wire test
    /// established). Never an adapter toggle.
    @MainActor
    private func toggleDisclosure(_ triangle: XCUIElement, expect: Int) -> Bool {
        let frame = triangle.frame
        guard triangle.exists, frame.width > 52, frame.height > 0, frame.width.isFinite, frame.height.isFinite else {
            XCTFail("disclosure triangle geometry unavailable: \(triangle)")
            return false
        }
        triangle.coordinate(withNormalizedOffset: CGVector(dx: 26 / frame.width, dy: 0.5)).click()
        return waitUntil { Self.disclosureValue(triangle) == expect }
    }

    @MainActor
    private func openSettings(_ app: XCUIApplication) -> XCUIElement? {
        let onboarding = app.windows["Welcome to Softmax"]
        guard onboarding.waitForExistence(timeout: 10) else { XCTFail("onboarding did not launch"); return nil }
        onboarding.buttons["onboarding.open_settings"].click()
        let settings = app.windows["Softmax Settings"]
        guard settings.waitForExistence(timeout: 5) else { XCTFail("settings did not open"); return nil }
        return settings
    }

    @MainActor
    private func openPalette(_ app: XCUIApplication) -> XCUIElement? {
        let onboarding = app.windows["Welcome to Softmax"]
        guard onboarding.waitForExistence(timeout: 10) else { XCTFail("onboarding did not launch"); return nil }
        onboarding.typeKey("k", modifierFlags: .command)
        let palette = app.descendants(matching: .any).matching(identifier: "Softmax Commands").firstMatch
        guard palette.waitForExistence(timeout: 5) else { XCTFail("command palette did not open"); return nil }
        return palette
    }

    @MainActor
    private func enableOrb(_ app: XCUIApplication) -> Bool {
        guard let settings = openSettings(app) else { return false }
        let toggle = settings.checkBoxes["floatingOrbToggle"]
        guard toggle.waitForExistence(timeout: 5) else { XCTFail("floating orb toggle missing"); return false }
        toggle.click()
        return app.buttons["Open Softmax chat"].waitForExistence(timeout: 5)
    }

    // MARK: - Source-bound guards (read the repo files this test was built from)

    private func sourceText(_ name: String) -> String? {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent(name)
        return try? String(contentsOf: url, encoding: .utf8)
    }

    private func comparatorSource(in text: String) -> String? {
        let marker = "    private func catalogMembershipFailures(actual: [String], registered: Set<String>) -> [String] {"
        guard let start = text.range(of: marker),
              let end = text.range(of: "\n    }\n", range: start.upperBound..<text.endIndex) else { return nil }
        return String(text[start.lowerBound..<end.upperBound])
    }

    /// The comparator is not forked, and the shared negative controls behave as
    /// in the baseline (unregistered, duplicate, missing-ax, empty id).
    func testCoverageComparatorIsIdenticalToBaseline() {
        guard let baseline = sourceText("SoftmaxUITests.swift"), let mine = sourceText("CatalogCoverageExtensionTests.swift"),
              let baselineBody = comparatorSource(in: baseline), let myBody = comparatorSource(in: mine) else {
            XCTFail("comparator source unreadable; parity is unproven"); return
        }
        XCTAssertEqual(myBody, baselineBody, "comparator drifted from the baseline census")
        let registered: Set<String> = ["a.one", "a.two"]
        XCTAssertEqual(catalogMembershipFailures(actual: ["a.one", "a.two"], registered: registered), [])
        XCTAssertEqual(catalogMembershipFailures(actual: ["a.one", "a.two", "a.three"], registered: registered), ["unregistered:a.three"])
        XCTAssertEqual(catalogMembershipFailures(actual: ["a.one"], registered: registered), ["missing-ax:a.two"])
        XCTAssertEqual(catalogMembershipFailures(actual: ["a.one", "a.one", "a.two"], registered: registered), ["duplicate:a.one"])
        XCTAssertEqual(catalogMembershipFailures(actual: ["a.one", "a.two", ""], registered: registered), ["empty-id", "unregistered:"])
    }

    /// Every one of the product's fourteen windows is accounted for exactly once.
    /// Nothing may be silently dropped, and unresolved ones carry a reason.
    func testCoverageLedgerAccountsForEveryAppWindow() {
        let baselineCovered: Set<String> = ["onboarding", "settings"]
        let extensionCovered: Set<String> = ["command_palette", "about", "keyboard_shortcuts", "face", "chat", "hub_pairing"]
        let unresolved: [String: String] = [
            "main": "no such window exists; AppSurface open/navigate return unsupported",
            "orb": "secondary gesture has a registry entry but no independent XCUI node; full gesture enumeration remains unqualified",
            "menu_bar": "MenuBarExtra popover is hosted by the status item; no ordinary XCUI route to its tree was established",
            "hud": "visible only while push-to-talk is listening/thinking/speaking (microphone + global key); no safe native driver",
            "sign_out_confirmation": "needs an enrolled hub identity (auth/pairing effect); sign out is sensitive",
            "quit_confirmation": "OS-owned NSAlert (runModal) shown only during an active conversation; quit is sensitive",
        ]
        guard let source = sourceText("../../Sources/SoftmaxApp/SoftmaxApp.swift"),
              let start = source.range(of: "private static let appWindows = ["),
              let end = source.range(of: "]", range: start.upperBound..<source.endIndex) else {
            XCTFail("appWindows source unreadable; ledger is unproven"); return
        }
        // Keep empty leading pieces so odd offsets are always the quoted names.
        let declared = source[start.upperBound..<end.lowerBound]
            .split(separator: "\"", omittingEmptySubsequences: false).enumerated()
            .filter { $0.offset % 2 == 1 }.map { String($0.element) }
        XCTAssertEqual(declared.count, 14)
        XCTAssertEqual(Set(declared).count, declared.count)
        XCTAssertTrue(baselineCovered.isDisjoint(with: extensionCovered))
        XCTAssertTrue(baselineCovered.union(extensionCovered).isDisjoint(with: Set(unresolved.keys)))
        XCTAssertEqual(baselineCovered.union(extensionCovered).union(Set(unresolved.keys)), Set(declared),
            "every product window must be covered or explicitly unresolved")
        XCTAssertEqual(baselineCovered.count + extensionCovered.count, 8)
        XCTAssertEqual(unresolved.count, 6)
        XCTAssertTrue(unresolved.values.allSatisfy { !$0.isEmpty })
    }

    // MARK: - command_palette

    @MainActor
    func testCatalogCoverageCommandPaletteStates() throws {
        let app = try launchProbed()
        defer { finish(app) }
        guard let palette = openPalette(app) else { return }
        let search = palette.textFields["Search commands and settings"]
        let command = { (id: String) in "command_palette.command." + id }
        let rows = ["settings-General", "settings-Hotkey", "settings-Audio", "settings-Hub", "settings-Coglets",
                    "settings-Updates", "settings-Privacy", "face", "chat", "voice", "about"]
        try captureCoverage(scope: palette, owner: "command_palette", route: "commands", tag: "default",
            sentinel: search, anchors: ["command_palette.search"] + rows.map(command),
            expecting: ["command_palette.search": (role: "text_field", verbs: ["type_text"], sensitive: false),
                        command("about"): (role: "button", verbs: ["press"], sensitive: false),
                        command("face"): (role: "button", verbs: ["press"], sensitive: false)])
        // Real keyboard filtering retires every non-matching row mount.
        palette.typeText("privacy")
        XCTAssertTrue(palette.buttons["About Softmax"].waitForNonExistence(timeout: 5))
        try captureCoverage(scope: palette, owner: "command_palette", route: "commands", tag: "filtered-privacy",
            sentinel: palette.buttons["Settings → Privacy"], anchors: ["command_palette.search", command("settings-Privacy")],
            absent: [command("about"), command("face"), command("settings-General")])
        palette.typeKey("a", modifierFlags: .command)
        palette.typeText("zz-no-matches")
        try captureCoverage(scope: palette, owner: "command_palette", route: "commands", tag: "no-matches",
            sentinel: palette.staticTexts["No matching commands or settings"], anchors: ["command_palette.search"],
            absent: rows.map(command))
        palette.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(palette.waitForNonExistence(timeout: 5))
        try assertNoVisibleRegistrations(window: "command_palette", tag: "escape")
    }

    // MARK: - about (+ expanded licence disclosure)

    @MainActor
    func testCatalogCoverageAboutAndLicenceDisclosure() throws {
        let app = try launchProbed()
        defer { finish(app) }
        guard let palette = openPalette(app) else { return }
        palette.typeText("about")
        XCTAssertTrue(palette.buttons["About Softmax"].waitForExistence(timeout: 5))
        palette.typeKey(.return, modifierFlags: [])
        let about = app.windows["About Softmax"]
        let sentinel = about.staticTexts["Softmax for Mac"]
        let base = ["about.copy_version", "about.support", "about.source", "about.licenses"]
        let expecting: [String: (role: String, verbs: [String], sensitive: Bool)] = [
            "about.copy_version": (role: "button", verbs: ["press"], sensitive: false),
            "about.licenses": (role: "disclosure", verbs: ["toggle"], sensitive: false)]
        try captureCoverage(scope: about, owner: "about", route: "about", tag: "collapsed",
            sentinel: sentinel, anchors: base, absent: ["about.sparkle_license"], expecting: expecting)
        let triangles = about.disclosureTriangles
        XCTAssertEqual(triangles.count, 1, "About must own exactly one native disclosure triangle")
        let triangle = triangles.firstMatch
        XCTAssertTrue(toggleDisclosure(triangle, expect: 1), "licence disclosure did not expand")
        try captureCoverage(scope: about, owner: "about", route: "about", tag: "expanded",
            sentinel: about.staticTexts.matching(NSPredicate(format: "label == %@",
                "Sparkle — MIT licence and third-party notices. Full Sparkle licence texts ship with this app. Softmax and Coglet source is available from the repository.")).firstMatch,
            anchors: base + ["about.sparkle_license"], expecting: expecting)
        XCTAssertTrue(toggleDisclosure(triangle, expect: 0), "licence disclosure did not collapse")
        XCTAssertTrue(about.descendants(matching: .any).matching(identifier: "about.sparkle_license")
            .firstMatch.waitForNonExistence(timeout: 5))
        try captureCoverage(scope: about, owner: "about", route: "about", tag: "collapsed-again",
            sentinel: sentinel, anchors: base, absent: ["about.sparkle_license"], expecting: expecting)
        // Links are never clicked (OpenURL would leave the app).
    }

    // MARK: - keyboard_shortcuts (Settings sheet)

    @MainActor
    func testCatalogCoverageKeyboardShortcutsSheet() throws {
        let app = try launchProbed()
        defer { finish(app) }
        guard let settings = openSettings(app) else { return }
        settings.buttons["settings.shortcuts.open"].click()
        let sheet = app.sheets.firstMatch
        try captureCoverage(scope: sheet, owner: "keyboard_shortcuts", route: "shortcuts", tag: "open",
            sentinel: sheet.staticTexts["Keyboard shortcuts"], anchors: ["keyboard_shortcuts.done"],
            expecting: ["keyboard_shortcuts.done": (role: "button", verbs: ["press"], sensitive: false)])
        sheet.buttons["keyboard_shortcuts.done"].click()
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5))
        try assertNoVisibleRegistrations(window: "keyboard_shortcuts", tag: "done")
    }

    // MARK: - face (borderless NSPanel; demo / compact dynamic states)

    @MainActor
    func testCatalogCoverageFacePanelStates() throws {
        let app = try launchProbed(["--show-face"])
        defer { finish(app) }
        let face = app.dialogs["Softmax Face"]
        let expecting = { (ids: [String]) -> [String: (role: String, verbs: [String], sensitive: Bool)] in
            Dictionary(uniqueKeysWithValues: ids.map { ($0, (role: "button", verbs: ["press"], sensitive: false)) })
        }
        let full = ["face.demo_toggle", "face.stop", "face.compact_toggle", "face.hide"]
        try captureCoverage(scope: face, owner: "face", route: "face", tag: "expanded",
            sentinel: face.buttons["face.compact_toggle"], anchors: full, expecting: expecting(full))
        face.buttons["face.demo_toggle"].click()
        try captureCoverage(scope: face, owner: "face", route: "face", tag: "demo-running",
            sentinel: face.staticTexts["Simulated demo · no audio"], anchors: full, expecting: expecting(full))
        face.buttons["face.compact_toggle"].click()
        XCTAssertTrue(face.buttons["face.demo_toggle"].waitForNonExistence(timeout: 5))
        try captureCoverage(scope: face, owner: "face", route: "face", tag: "compact",
            sentinel: face.buttons["face.compact_toggle"], anchors: ["face.compact_toggle", "face.hide"],
            absent: ["face.demo_toggle", "face.stop"], expecting: expecting(["face.compact_toggle", "face.hide"]))
        face.buttons["face.compact_toggle"].click()
        XCTAssertTrue(face.buttons["face.demo_toggle"].waitForExistence(timeout: 5))
        try captureCoverage(scope: face, owner: "face", route: "face", tag: "re-expanded",
            sentinel: face.buttons["face.compact_toggle"], anchors: full, expecting: expecting(full))
        face.buttons["face.hide"].click()
        XCTAssertTrue(face.waitForNonExistence(timeout: 5))
        try assertNoVisibleRegistrations(window: "face", tag: "hide")
    }


    // MARK: - chat / voice (titled NSPanel; SwiftUI Menu popup)

    @MainActor
    func testCatalogCoverageChatMenuAndVoiceRoutes() throws {
        let app = try launchProbed()
        defer { finish(app) }
        guard enableOrb(app) else { XCTFail("orb did not appear after the native Settings toggle"); return }
        let orbButton = app.buttons["orb.open_chat"]
        orbButton.click()
        let chat = app.dialogs["Softmax Conversation"]
        try captureCoverage(scope: chat, owner: "chat", route: "chat", tag: "chat",
            sentinel: chat.textViews["Message draft"], anchors: ["chat.mode", "chat.draft", "chat.send"],
            expecting: ["chat.mode": (role: "picker", verbs: ["select"], sensitive: false),
                        "chat.draft": (role: "text_editor", verbs: ["type_text"], sensitive: false),
                        "chat.send": (role: "button", verbs: ["press"], sensitive: false)])
        // Dynamic: the ellipsis Menu, opened by a real click. Registered side is
        // restricted to the two menu members; the AX side is the open menu's items.
        let popups = chat.popUpButtons, menuButtons = chat.menuButtons
        let trigger = popups.count > 0 ? popups.firstMatch : menuButtons.firstMatch
        if trigger.waitForExistence(timeout: 5) {
            trigger.click()
            let menu = trigger.menus.firstMatch
            try captureCoverage(scope: menu, owner: "chat", route: "chat", tag: "menu-open",
                sentinel: menu, anchors: ["chat.clear_draft", "chat.hide_orb"], types: [.menuItem],
                restrictRegisteredTo: ["chat.clear_draft", "chat.hide_orb"])
            app.typeKey(.escape, modifierFlags: [])
            XCTAssertTrue(menu.waitForNonExistence(timeout: 5))
            XCTAssertTrue(chat.exists, "dismissing the menu must not close the conversation")
            // Same owner and route before any secondary gesture: discriminate a close-time
            // leak from a later route-change remount, with the full strict census unchanged.
            try captureCoverage(scope: chat, owner: "chat", route: "chat", tag: "menu-closed",
                sentinel: chat.textViews["Message draft"], anchors: ["chat.mode", "chat.draft", "chat.send"],
                absent: ["chat.clear_draft", "chat.hide_orb"],
                expecting: ["chat.mode": (role: "picker", verbs: ["select"], sensitive: false),
                            "chat.draft": (role: "text_editor", verbs: ["type_text"], sensitive: false),
                            "chat.send": (role: "button", verbs: ["press"], sensitive: false)])
        } else {
            XCTFail("conversation menu trigger not exposed by the AX tree: \(chat.debugDescription)")
        }
        // Voice route via the orb's real secondary gesture. No permission button is pressed.
        orbButton.rightClick()
        try captureCoverage(scope: chat, owner: "chat", route: "voice", tag: "voice",
            sentinel: chat.staticTexts["Use the push-to-talk hotkey to capture audio. Runtime speech delivery is not connected yet."],
            anchors: ["chat.mode", "chat.voice.microphone_settings", "chat.voice.refresh_permissions"],
            absent: ["chat.draft", "chat.send"],
            expecting: ["chat.voice.microphone_settings": (role: "button", verbs: ["press"], sensitive: true),
                        "chat.voice.refresh_permissions": (role: "button", verbs: ["press"], sensitive: false)])
    }

    // MARK: - hub_pairing (sheet; expanded manual disclosure) from both hosts

    @MainActor
    private func pairingSheetCoverage(_ app: XCUIApplication, prefix: String, host: String) throws {
        let sheet = app.sheets.firstMatch
        let code = prefix + ".pairing.code", manualID = prefix + ".pairing.manual"
        let fixed = [code, manualID, prefix + ".pairing.import_qr", prefix + ".pairing.cancel", prefix + ".pairing.connect"]
        let manualFields = ["router_name", "router_node", "router_addresses", "relay_url", "invitation"]
            .map { prefix + ".pairing." + $0 }
        let verbs: (String, String, Bool) -> (role: String, verbs: [String], sensitive: Bool) = { ($0, [$1], $2) }
        let expecting: [String: (role: String, verbs: [String], sensitive: Bool)] = [
            code: verbs("secure_field", "type_text", true), manualID: verbs("disclosure", "toggle", true),
            prefix + ".pairing.import_qr": verbs("button", "press", true), prefix + ".pairing.cancel": verbs("button", "press", false),
            prefix + ".pairing.connect": verbs("button", "press", true)]
        try captureCoverage(scope: sheet, owner: "hub_pairing", route: "code", tag: host + "-collapsed",
            sentinel: sheet.staticTexts["Pair router"], anchors: fixed, absent: manualFields, expecting: expecting)
        // Expanded manual disclosure through its real native chevron (no adapter toggle).
        let identified = sheet.disclosureTriangles[manualID]
        let triangle = identified.exists ? identified
            : sheet.descendants(matching: .any).matching(identifier: manualID).firstMatch.disclosureTriangles.firstMatch
        guard toggleDisclosure(triangle, expect: 1) else { XCTFail("manual disclosure did not expand: " + host); return }
        XCTAssertEqual(sheet.textFields.matching(identifier: manualID).count, 0)
        XCTAssertEqual(sheet.secureTextFields.matching(identifier: manualID).count, 0)
        var manualExpecting = expecting
        for id in manualFields {
            manualExpecting[id] = verbs(id.hasSuffix("invitation") ? "secure_field" : "text_field", "type_text", true)
        }
        try captureCoverage(scope: sheet, owner: "hub_pairing", route: "manual", tag: host + "-expanded",
            sentinel: sheet.textFields[prefix + ".pairing.router_name"], anchors: fixed + manualFields,
            expecting: manualExpecting)
        guard toggleDisclosure(triangle, expect: 0) else { XCTFail("manual disclosure did not collapse: " + host); return }
        XCTAssertTrue(sheet.textFields[prefix + ".pairing.router_name"].waitForNonExistence(timeout: 5))
        try captureCoverage(scope: sheet, owner: "hub_pairing", route: "code", tag: host + "-collapsed-again",
            sentinel: sheet.staticTexts["Pair router"], anchors: fixed, absent: manualFields, expecting: expecting)
        // Nothing is typed, imported or connected: Cancel is the only action taken.
        sheet.buttons[prefix + ".pairing.cancel"].click()
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5))
        try assertNoVisibleRegistrations(window: "hub_pairing", tag: host + "-cancel")
    }

    @MainActor
    func testCatalogCoveragePairingSheetFromOnboardingAndSettings() throws {
        let app = try launchProbed()
        defer { finish(app) }
        let onboarding = app.windows["Welcome to Softmax"]
        XCTAssertTrue(onboarding.waitForExistence(timeout: 10))
        onboarding.buttons["onboarding.continue"].click()
        guard onboarding.staticTexts["Let Softmax hear you."].waitForExistence(timeout: 5) else {
            XCTFail("onboarding permissions step did not appear"); return
        }
        onboarding.buttons["onboarding.continue"].click()
        let onboardingPair = onboarding.buttons["onboarding.hub.pair"]
        guard onboardingPair.waitForExistence(timeout: 5) else { XCTFail("onboarding pairing entry missing"); return }
        onboardingPair.click()
        try pairingSheetCoverage(app, prefix: "onboarding.hub", host: "onboarding")
        onboarding.buttons["onboarding.open_settings"].click()
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        settings.buttons["settings.tab.hub"].click()
        let settingsPair = settings.buttons["settings.hub.pair"]
        guard settingsPair.waitForExistence(timeout: 5) else { XCTFail("settings pairing entry missing"); return }
        settingsPair.click()
        try pairingSheetCoverage(app, prefix: "settings.hub", host: "settings")
    }
}
