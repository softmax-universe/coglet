import XCTest
import Darwin
import CogletTypes
import CoreFoundation

private struct WireTestFailure: Swift.Error {}

/// A separate process runs coglet-mock's actual runtime-side SDK peer. The
/// callback drains stdout independently of XCUI waits; bounded parsing fails
/// closed and keeps raw bytes for attached evidence.
private final class AppMockPeer: @unchecked Sendable {
    let process = Process()
    private let input = Pipe(), output = Pipe()
    private let lock = NSLock()
    private var pending = Data(), raw = Data()
    private var frames: [[String: Any]] = []
    private var invalid = false, outputEnded = false
    init(executable: String, socket: String, errors: URL) throws {
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = [socket]
        process.standardInput = input; process.standardOutput = output
        FileManager.default.createFile(atPath: errors.path, contents: nil)
        process.standardError = try FileHandle(forWritingTo: errors)
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            if data.isEmpty {
                self.lock.lock(); self.outputEnded = true; self.lock.unlock()
                handle.readabilityHandler = nil; return
            }
            self.consume(data)
        }
        try process.run()
    }
    private func consume(_ bytes: Data) {
        lock.lock(); defer { lock.unlock() }
        guard !invalid, raw.count + bytes.count <= 1_048_576 else { invalid = true; return }
        raw.append(bytes); pending.append(bytes)
        while let end = pending.firstIndex(of: 10) {
            let line = pending.prefix(upTo: end)
            pending.removeSubrange(...end)
            guard frames.count < 512,
                  let frame = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] else {
                invalid = true; return
            }
            frames.append(frame)
        }
    }
    func snapshot() throws -> [[String: Any]] {
        lock.lock(); defer { lock.unlock() }
        guard !invalid else { throw WireTestFailure() }
        return frames
    }
    func bytes() -> Data { lock.lock(); defer { lock.unlock() }; return raw }
    func drained() -> Bool { lock.lock(); defer { lock.unlock() }; return outputEnded && pending.isEmpty && !invalid }
    func send(_ command: [String: Any]) throws {
        guard process.isRunning else { throw WireTestFailure() }
        var data = try JSONSerialization.data(withJSONObject: command, options: [.sortedKeys])
        data.append(10)
        try input.fileHandleForWriting.write(contentsOf: data)
    }
    func terminate() {
        output.fileHandleForReading.readabilityHandler = nil
        try? input.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
    }
}

final class AppSurfaceWireTests: XCTestCase {
    @MainActor
    private func wait(_ predicate: @escaping @MainActor () -> Bool, timeout: TimeInterval = 8) throws {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            MainActor.assumeIsolated { predicate() }
        }, object: nil)
        guard XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed else {
            XCTFail("live app wire proof timed out"); throw WireTestFailure()
        }
    }
    // Only XCTest's element-address slot is unstable. Preserve identifiers,
    // geometry, values, enabled state, focus and hex-looking semantic content.
    private func canonicalAX(_ tree: String) -> String {
        tree.replacingOccurrences(of: #"(?m)(^[ \t]*(?:Attributes: )?(?:→[ \t]*)?[A-Za-z][A-Za-z0-9 ()]*, )0x[0-9a-fA-F]+(?=, (?:\{\{|pid:))"#,
            with: "$1<address>", options: .regularExpression)
    }
    private func containsCanary(_ bytes: Data, canaries: [String]) -> Bool {
        canaries.contains { bytes.range(of: Data($0.utf8)) != nil }
    }
    // Fail closed on schema drift or arbitrary error/native text in diagnostics.
    private func closedDiagnostic(_ bytes: Data) -> Bool {
        guard bytes.count <= 4096,
              let value = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any],
              Set(value.keys) == ["phase", "category", "native"] || Set(value.keys) == ["phase", "category"],
              let phase = value["phase"] as? String, let category = value["category"] as? String else { return false }
        let phases: Set<String> = ["prepare", "accept", "ledger", "sessionStart", "transportEnqueue", "transportFlush",
            "transportReceive", "sdkReceive", "activate", "nativeCheck", "nativeAttach", "nativeRefresh", "adapterCapture",
            "nativePublish", "nativeObserve", "nativeWitness", "sdkValidate", "sdkReconcile", "sdkEvents", "sdkTick",
            "statusWrite", "wait", "cancelled"]
        let categories: Set<String> = ["registrationLatch", "retired", "notTesting", "runtimeActive", "invariant", "cancelled",
            "registryConflict", "registryInvalidRegistration", "registryDuplicate", "registryUnavailable",
            "registryPermissionDenied", "registryUnsupported", "registryExhausted", "adapterConflict", "adapterUnavailable",
            "adapterInvalidArguments", "adapterPermissionDenied", "adapterUnsupported", "adapterExhausted", "adapterInternal", "surfaceClosed",
            "surfaceInvalidFrame", "surfaceProtocol", "surfaceSystem", "surfaceUnauthenticated", "surfaceExhausted", "other"]
        guard phases.contains(phase), categories.contains(category) else { return false }
        if let native = value["native"] {
            guard let context = native as? [String: Any], Set(context.keys) == ["hasUnqualifiedAppModal", "modalWindowPresent",
                "visibleAttachedSheet", "pairingOwnerMatches", "pairingRoutePresent", "pairingOwnerVisible", "registrationFailed"] else { return false }
            return context.values.allSatisfy {
                guard let number = $0 as? NSNumber else { return false }
                return CFGetTypeID(number) == CFBooleanGetTypeID()
            }
        }
        return true
    }
    func testClosedFailureDiagnosticSchema() throws {
        let context: [String: Any] = ["hasUnqualifiedAppModal": true, "modalWindowPresent": false,
            "visibleAttachedSheet": true, "pairingOwnerMatches": false, "pairingRoutePresent": true,
            "pairingOwnerVisible": false, "registrationFailed": true]
        let valid: [String: Any] = ["phase": "nativeCheck", "category": "registrationLatch", "native": context]
        func encoded(_ value: [String: Any]) throws -> Data { try JSONSerialization.data(withJSONObject: value) }
        XCTAssertTrue(closedDiagnostic(try encoded(valid)))
        XCTAssertTrue(closedDiagnostic(try encoded(["phase": "transportReceive", "category": "surfaceClosed"])))
        for key in ["phase", "category", "description"] {
            var invalid = valid; invalid[key] = "untrusted-text"
            XCTAssertFalse(closedDiagnostic(try encoded(invalid)))
        }
        for replacement in ["untrusted-text", 1, NSNull()] as [Any] {
            var invalid = valid, changed = context; changed["pairingOwnerMatches"] = replacement; invalid["native"] = changed
            XCTAssertFalse(closedDiagnostic(try encoded(invalid)))
        }
        var invalid = valid; var changed = context; changed["extra"] = false; invalid["native"] = changed
        XCTAssertFalse(closedDiagnostic(try encoded(invalid)))
        XCTAssertFalse(closedDiagnostic(Data(repeating: 32, count: 4097)))
        XCTAssertFalse(closedDiagnostic(Data("not-json".utf8)))
    }
    func testEvidenceCanonicalizationAndCanaryDetection() {
        let first = "  TextField, 0x600abc, {{1, 2}, {30, 40}}, identifier: 'semantic-0x123', value: 'old-0x456', Disabled, Focused"
        let second = "  TextField, 0x700def, {{1, 2}, {30, 40}}, identifier: 'semantic-0x123', value: 'old-0x456', Disabled, Focused"
        XCTAssertTrue(canonicalAX(first) == canonicalAX(second), "object addresses must not affect equality")
        for changed in [second.replacingOccurrences(of: "old-0x456", with: "new-0x456"),
            second.replacingOccurrences(of: "semantic-0x123", with: "semantic-0x124"),
            second.replacingOccurrences(of: "{30, 40}", with: "{31, 40}"),
            second.replacingOccurrences(of: ", Disabled", with: ""),
            second.replacingOccurrences(of: ", Focused", with: "")] {
            XCTAssertFalse(canonicalAX(first) == canonicalAX(changed), "semantic AX changes must remain visible")
        }
        // Actual parent-run formats: a PID-bearing Attributes header, arrow
        // path root and parenthesized main-window element. Not just TextField.
        let actualFormats = """
        Attributes: Application, 0x737642800, pid: 60015, title: 'Softmax', Disabled
         →Application, 0x737642800, pid: 60015, title: 'Softmax', Disabled
            Window (Main), 0x737643200, {{132.0, 57.0}, {760.0, 572.0}}, identifier: 'softmax.settings', title: 'Softmax Settings', Disabled
        """
        let movedObjects = actualFormats.replacingOccurrences(of: "0x737642800", with: "0x837642800")
            .replacingOccurrences(of: "0x737643200", with: "0x837643200")
        XCTAssertTrue(canonicalAX(actualFormats) == canonicalAX(movedObjects), "actual AX address slots must be stable")
        XCTAssertFalse(canonicalAX(actualFormats) == canonicalAX(movedObjects.replacingOccurrences(of: "pid: 60015", with: "pid: 60016")),
            "process identity must remain visible")
        XCTAssertFalse(canonicalAX(actualFormats) == canonicalAX(movedObjects.replacingOccurrences(of: "softmax.settings", with: "softmax.changed")),
            "window identity must remain visible")
        let synthetic = ["synthetic-marker-one", "synthetic-marker-two"]
        for marker in synthetic {
            XCTAssertTrue(containsCanary(Data(("prefix" + marker + "suffix").utf8), canaries: synthetic),
                "scanner positive control failed")
        }
        XCTAssertFalse(containsCanary(Data("unrelated evidence".utf8), canaries: synthetic),
            "scanner negative control failed")
    }
    private func body(_ frame: [String: Any]) -> [String: Any] { frame["body"] as? [String: Any] ?? [:] }
    private func payload(_ frame: [String: Any]) -> [String: Any] { body(frame)["payload"] as? [String: Any] ?? [:] }
    private func id() -> String { UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() }
    @MainActor
    func test_softmax_app_observe_views_and_focus() throws {
        // No skip in the required native lane, no shell/cargo inside the runner.
        let executable = try XCTUnwrap(ProcessInfo.processInfo.environment["SOFTMAX_APP_MOCK_PEER"])
        XCTAssertTrue(FileManager.default.isExecutableFile(atPath: executable))
        let temp = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
        let directory = temp.appendingPathComponent("sw-" + String(UUID().uuidString.prefix(8)))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--sdk-wire-proof-directory", directory.path]
        app.launchEnvironment["TMPDIR"] = temp.path
        app.launch()
        defer { app.terminate() }
        // Retained attachment guard only: no credential/secure-input fixture is
        // seeded in this slice, and this test makes no credential-secrecy claim.
        let secretCanaries = [id() + id(), id() + id()]
        var retainedFinalLedger = false
        func safeAttachmentBytes(_ bytes: Data) -> Data {
            let leaked = containsCanary(bytes, canaries: secretCanaries)
            XCTAssertFalse(leaked, "generated secret leaked into evidence")
            return leaked ? Data("Evidence withheld: generated secret detected.".utf8) : bytes
        }
        // Installed before socket/peer readiness, so early throws retain evidence.
        // Observe only: never close the session, terminate twice or relax gates.
        defer {
            if let bytes = try? Data(contentsOf: directory.appendingPathComponent("failure.json")) {
                let safe = safeAttachmentBytes(bytes)
                let valid = closedDiagnostic(safe)
                XCTAssertTrue(valid, "wire failure diagnostic must have a closed schema")
                let attachment = XCTAttachment(data: valid ? safe : Data("Diagnostic withheld: invalid schema.".utf8),
                    uniformTypeIdentifier: "public.json")
                attachment.name = "live-app-wire-failure-diagnostic"; attachment.lifetime = .keepAlways; add(attachment)
            }
            if let bytes = try? Data(contentsOf: directory.appendingPathComponent("status")),
               bytes == Data("active".utf8) || bytes == Data("closed".utf8) || bytes == Data("close_failed".utf8) {
                let attachment = XCTAttachment(data: safeAttachmentBytes(bytes), uniformTypeIdentifier: "public.plain-text")
                attachment.name = "live-app-wire-terminal-status"; attachment.lifetime = .keepAlways; add(attachment)
                // Preserve only an actually closed, decoded durable ledger. The
                // successful path retains its existing assertions/attachment.
                if !retainedFinalLedger, bytes == Data("closed".utf8),
                   let ledger = try? Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor")),
                   let saved = try? CogletTypes.CBORCodec.decodeProfile(Array(ledger)),
                   case .map(let entries) = saved,
                   entries.first(where: { $0.key == "running" })?.value == .boolean(false),
                   entries.first(where: { $0.key == "killed" })?.value == .boolean(false) {
                    let retained = XCTAttachment(data: safeAttachmentBytes(ledger), uniformTypeIdentifier: "public.data")
                    retained.name = "live-app-wire-failure-closed-ledger"; retained.lifetime = .keepAlways; add(retained)
                }
            }
        }
        let pathFile = directory.appendingPathComponent("socket-path")
        try wait { FileManager.default.fileExists(atPath: pathFile.path) }
        let socket = try String(contentsOf: pathFile, encoding: .utf8)
        XCTAssertLessThan(socket.utf8.count, 100)
        let errorFile = directory.appendingPathComponent("peer-stderr")
        let peer = try AppMockPeer(executable: executable, socket: socket, errors: errorFile)
        defer {
            let raw = XCTAttachment(data: safeAttachmentBytes(peer.bytes()), uniformTypeIdentifier: "public.jsonl")
            raw.name = "live-app-coglet-mock-wire"; raw.lifetime = .keepAlways; add(raw)
            if let bytes = try? Data(contentsOf: errorFile) {
                let errors = XCTAttachment(data: safeAttachmentBytes(bytes), uniformTypeIdentifier: "public.plain-text")
                errors.name = "live-app-coglet-mock-stderr"; errors.lifetime = .keepAlways; add(errors)
            }
            let tree = XCTAttachment(data: safeAttachmentBytes(Data(app.debugDescription.utf8)), uniformTypeIdentifier: "public.plain-text")
            tree.name = "live-app-wire-final-ax"; tree.lifetime = .keepAlways; add(tree)
            peer.terminate()
        }
        // Snapshot parse/overflow failures propagate, never become a timeout.
        func waitFrames(_ predicate: ([[String: Any]]) -> Bool) throws {
            let deadline = Date().addingTimeInterval(8)
            repeat {
                if predicate(try peer.snapshot()) { return }
                RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.02)))
            } while Date() < deadline
            XCTFail("live peer response timed out"); throw WireTestFailure()
        }
        try waitFrames { $0.contains { $0["type"] as? String == "grants" } }
        XCTAssertTrue(peer.process.isRunning)
        func event(_ kind: String, after mark: Int, matching: @escaping ([String: Any]) -> Bool) throws -> [String: Any] {
            func matches(_ frame: [String: Any]) -> Bool {
                frame["type"] as? String == "event" && body(frame)["kind"] as? String == kind && matching(payload(frame))
            }
            try waitFrames { $0.dropFirst(mark).contains(where: matches) }
            return try XCTUnwrap(peer.snapshot().dropFirst(mark).last(where: matches))
        }
        @discardableResult
        func act(_ operation: String, _ args: [String: Any]) throws -> String {
            // One identity per effect. No new-id retry on conflict or refusal.
            let actionID = id()
            try peer.send(["kind": "act", "operation": operation, "args": args, "id": actionID])
            func terminal(_ frame: [String: Any]) -> Bool {
                (frame["type"] as? String == "accept" && body(frame)["id"] as? String == actionID
                    && body(frame)["accepted"] as? Bool == false)
                || (body(frame)["kind"] as? String == "action.outcome" && payload(frame)["id"] as? String == actionID)
            }
            try waitFrames { $0.contains(where: terminal) }
            let frame = try XCTUnwrap(peer.snapshot().first(where: terminal))
            guard payload(frame)["status"] as? String == "succeeded" else {
                XCTFail("foundation live action was refused or failed"); throw WireTestFailure()
            }
            return actionID
        }
        var refusedActions: [String: String] = [:]
        func assertRefused(_ actionID: String, reason: String, in frames: [[String: Any]]) throws {
            let rejects = frames.filter { $0["type"] as? String == "accept"
                && body($0)["id"] as? String == actionID && body($0)["accepted"] as? Bool == false }
            let outcomes = frames.filter { $0["type"] as? String == "event"
                && body($0)["kind"] as? String == "action.outcome" && payload($0)["id"] as? String == actionID }
            XCTAssertEqual(rejects.count + outcomes.count, 1, "one terminal refusal per attempted effect")
            for frame in rejects + outcomes {
                let receipt = frame["type"] as? String == "accept" ? body(frame) : payload(frame)
                if frame["type"] as? String == "event" {
                    XCTAssertEqual(receipt["status"] as? String, "failed")
                }
                let error = try XCTUnwrap(receipt["error"] as? [String: Any])
                XCTAssertEqual(error["code"] as? String, reason)
                XCTAssertEqual(error["message"] as? String, reason)
                XCTAssertEqual(error["retryable"] as? Bool, false)
            }
            XCTAssertFalse(outcomes.contains { payload($0)["status"] as? String == "succeeded" })
        }
        func refuse(_ operation: String, _ args: [String: Any], reason: String) throws {
            // Distinct identity for each attempted effect; never retry a refusal.
            let actionID = id()
            XCTAssertNil(refusedActions.updateValue(reason, forKey: actionID))
            try peer.send(["kind": "act", "operation": operation, "args": args, "id": actionID])
            try waitFrames { frames in
                frames.contains { frame in
                    (frame["type"] as? String == "accept" && body(frame)["id"] as? String == actionID
                        && body(frame)["accepted"] as? Bool == false)
                    || (frame["type"] as? String == "event" && body(frame)["kind"] as? String == "action.outcome"
                        && payload(frame)["id"] as? String == actionID)
                }
            }
            try assertRefused(actionID, reason: reason, in: peer.snapshot())
        }
        var readsRemaining = 96 // readiness samples and every controls page
        func read(_ operation: String, _ args: [String: Any]) throws -> [String: Any] {
            guard readsRemaining > 0 else { XCTFail("read budget exhausted"); throw WireTestFailure() }
            readsRemaining -= 1
            let readID = id()
            try peer.send(["kind": "read", "operation": operation, "args": args, "id": readID])
            func matches(_ frame: [String: Any]) -> Bool {
                frame["type"] as? String == "read_reply" && body(frame)["correlation_id"] as? String == readID
            }
            try waitFrames { $0.contains(where: matches) }
            let reply = try XCTUnwrap(peer.snapshot().first(where: matches))
            guard body(reply)["ok"] as? Bool == true else {
                XCTFail("foundation wire read refused"); throw WireTestFailure()
            }
            return try XCTUnwrap(body(reply)["value"] as? [String: Any])
        }
        func state() throws -> [String: Any] { try read("softmax.app.state", [:]) }
        func readControls(_ window: String) throws -> [[String: Any]] {
            var rows: [[String: Any]] = [], seen = Set<String>(), cursors = Set<String>()
            var after: Any = NSNull(), revision: Any = NSNull()
            for _ in 0..<32 {
                // Stale/conflict fails, never restart with mixed revisions.
                let page = try read("softmax.app.controls", ["window": window, "after": after,
                    "limit": 128, "revision": revision])
                XCTAssertEqual(page["window"] as? String, window)
                let current = try XCTUnwrap(page["revision"] as? NSNumber)
                if let expected = revision as? NSNumber { XCTAssertEqual(current, expected) }
                revision = current
                let batch = try XCTUnwrap(page["controls"] as? [[String: Any]])
                XCTAssertLessThanOrEqual(batch.count, 128)
                for row in batch {
                    let key = try XCTUnwrap(row["id"] as? String)
                    guard seen.insert(key).inserted else {
                        XCTFail("duplicate wire control"); throw WireTestFailure()
                    }
                    rows.append(row)
                }
                guard let next = page["next"] else { throw WireTestFailure() }
                if next is NSNull { return rows }
                let cursor = try XCTUnwrap(next as? String)
                guard cursors.insert(cursor).inserted else { throw WireTestFailure() }
                after = cursor
            }
            XCTFail("controls pagination exceeded bound"); throw WireTestFailure()
        }
        // Fresh sampling outside XCTest predicates: no nested waiters, action
        // retries or swallowed read errors while native registration settles.
        func observe(route: String, focused: Bool = false) throws -> [String: Any] {
            let deadline = Date().addingTimeInterval(8)
            for attempt in 0..<12 {
                let snapshot = try state()
                let windows = try XCTUnwrap(snapshot["windows"] as? [[String: Any]])
                if windows.contains(where: { $0["window"] as? String == "settings"
                    && $0["open"] as? Bool == true && $0["route"] as? String == route }) {
                    let rows = try readControls("settings")
                    let search = rows.first { $0["id"] as? String == "settings.search" }
                    let focus = snapshot["focus"] as? [String: Any]
                    if search != nil && (!focused || (focus?["window"] as? String == "settings"
                        && focus?["control"] as? String == "settings.search")) { return snapshot }
                }
                guard attempt < 11, Date() < deadline else { break }
                RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.25)))
            }
            XCTFail("fresh app observation did not settle within bound"); throw WireTestFailure()
        }
        func canonical(_ value: Any) throws -> Data {
            try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        }
        // Navigate changes an OPEN window's route; it must not implicitly open it.
        // Retain a wire-state witness for the closed-window precondition that
        // caused the original native failure, then prove the actual open path.
        func assertMainAbsent(_ snapshot: [String: Any]) throws {
            let windows = try XCTUnwrap(snapshot["windows"] as? [[String: Any]])
            let main = windows.filter { $0["window"] as? String == "main" }
            XCTAssertEqual(main.count, 1, "main is one explicit closed record, not omitted or duplicated")
            let record = try XCTUnwrap(main.first)
            XCTAssertEqual(record["open"] as? Bool, false)
            XCTAssertTrue(record["route"] is NSNull)
            XCTAssertTrue(try readControls("main").isEmpty)
            XCTAssertEqual(app.windows.matching(identifier: "softmax.main").count, 0)
        }
        let capabilityFrames = try peer.snapshot().filter { $0["type"] as? String == "capabilities" }
        XCTAssertEqual(capabilityFrames.count, 1)
        let advertisedEntries = try XCTUnwrap(body(XCTUnwrap(capabilityFrames.first))["entries"] as? [[String: Any]])
        let advertisedActions = try advertisedEntries.flatMap { try XCTUnwrap($0["actions"] as? [String]) }
        let grantFrames = try peer.snapshot().filter { $0["type"] as? String == "grants" }
        XCTAssertEqual(grantFrames.count, 1)
        let grantEntries = try XCTUnwrap(body(XCTUnwrap(grantFrames.first))["entries"] as? [[String: Any]])
        XCTAssertEqual(grantEntries.count, 1)
        let grantedActions = try XCTUnwrap(XCTUnwrap(grantEntries.first)["operations"] as? [String])
        for operation in ["softmax.app.open_window", "softmax.app.navigate", "softmax.app.close_window"] {
            XCTAssertTrue(advertisedActions.contains(operation))
            XCTAssertTrue(grantedActions.contains(operation))
        }
        let initial = try state()
        try assertMainAbsent(initial)
        // Trace: AppDelegate has no main window. openHost throws unsupported,
        // mapped by AppSurfaceMailbox.failure to wire unavailable. Navigate is
        // unavailable at the adapter's closed-window guard. A closed close is
        // deliberately a successful no-op; it never calls closeHost.
        try refuse("softmax.app.open_window", ["window": "main"], reason: "unavailable")
        try assertMainAbsent(state())
        try refuse("softmax.app.navigate", ["window": "main", "route": "main"], reason: "unavailable")
        try assertMainAbsent(state())
        try act("softmax.app.close_window", ["window": "main"])
        try assertMainAbsent(state())
        // Ordinary Settings success below proves liveness after the refusals.
        let initialWindows = try XCTUnwrap(initial["windows"] as? [[String: Any]])
        let initialSettings = try XCTUnwrap(initialWindows.first { $0["window"] as? String == "settings" })
        XCTAssertEqual(initialSettings["open"] as? Bool, false)
        let openMark = try peer.snapshot().count
        try act("softmax.app.open_window", ["window": "settings"])
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        _ = try event("softmax.app.window_changed", after: openMark) {
            $0["window"] as? String == "settings" && $0["open"] as? Bool == true
        }
        let opened = try state()
        let openedWindows = try XCTUnwrap(opened["windows"] as? [[String: Any]])
        let openedSettings = try XCTUnwrap(openedWindows.first { $0["window"] as? String == "settings" })
        XCTAssertEqual(openedSettings["open"] as? Bool, true)
        // Visit another real route so the following navigation MUST emit
        // view_changed, not merely an initial window_changed.
        _ = try observe(route: "general")
        try act("softmax.app.navigate", ["window": "settings", "route": "audio"])
        XCTAssertTrue(settings.checkBoxes["settings.audio.sound_cues"].waitForExistence(timeout: 5))
        _ = try observe(route: "audio")
        let navigateMark = try peer.snapshot().count
        try act("softmax.app.navigate", ["window": "settings", "route": "general"])
        let navigation = try event("softmax.app.view_changed", after: navigateMark) {
            $0["window"] as? String == "settings" && $0["route"] as? String == "general"
        }
        _ = try observe(route: "general")
        let search = settings.textFields["settings.search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        let focusMark = try peer.snapshot().count
        let focusID = try act("softmax.app.set_focus", ["window": "settings", "control": "settings.search"])
        _ = try event("softmax.app.focus_changed", after: focusMark) {
            $0["window"] as? String == "settings" && $0["control"] as? String == "settings.search"
        }
        try wait { search.value(forKey: "hasKeyboardFocus") as? Bool == true }
        let focused = try observe(route: "general", focused: true)
        XCTAssertGreaterThanOrEqual(try XCTUnwrap(focused["revision"] as? NSNumber).uint64Value,
            try XCTUnwrap(payload(navigation)["revision"] as? NSNumber).uint64Value)
        func focusOutcomes(_ frames: [[String: Any]]) -> [[String: Any]] {
            frames.filter { body($0)["kind"] as? String == "action.outcome" && payload($0)["id"] as? String == focusID }
        }
        let original = try focusOutcomes(peer.snapshot())
        XCTAssertEqual(original.count, 1)
        let originalReceipt = try canonical(payload(XCTUnwrap(original.first)))
        func focusAccepts(_ frames: [[String: Any]]) -> [[String: Any]] {
            frames.filter { $0["type"] as? String == "accept" && body($0)["id"] as? String == focusID }
        }
        let accepted = try focusAccepts(peer.snapshot())
        XCTAssertEqual(accepted.count, 1)
        let originalAccept = try canonical(body(XCTUnwrap(accepted.first)))
        // The peer retains and resends the exact original set_focus envelope.
        try peer.send(["kind": "retry", "id": focusID])
        try waitFrames { focusOutcomes($0).count == 2 && focusAccepts($0).count == 2 }
        let replayed = try focusOutcomes(peer.snapshot())
        // Receipts are identical; event seq/logical_time are freshly allocated
        // by the SDK for replay and therefore are not part of receipt equality.
        XCTAssertEqual(try canonical(payload(XCTUnwrap(replayed.last))), originalReceipt)
        XCTAssertEqual(try canonical(body(XCTUnwrap(focusAccepts(peer.snapshot()).last))), originalAccept)
        try wait { search.value(forKey: "hasKeyboardFocus") as? Bool == true }
        _ = try observe(route: "general", focused: true)
        let closeMark = try peer.snapshot().count
        try act("softmax.app.close_window", ["window": "settings"])
        _ = try event("softmax.app.window_changed", after: closeMark) {
            $0["window"] as? String == "settings" && $0["open"] as? Bool == false && $0["route"] is NSNull
        }
        try wait { !settings.exists }
        let closedWindows = try XCTUnwrap(try state()["windows"] as? [[String: Any]])
        XCTAssertEqual(closedWindows.first { $0["window"] as? String == "settings" }?["open"] as? Bool, false)
        let grants = try peer.snapshot().filter { $0["type"] as? String == "grants" }
        XCTAssertEqual(grants.count, 1)
        XCTAssertGreaterThan(try XCTUnwrap(body(XCTUnwrap(grants.first))["epoch"] as? NSNumber).uint64Value, 0)
        try peer.send(["kind": "close"])
        try wait { !peer.process.isRunning }
        XCTAssertEqual(peer.process.terminationStatus, 0)
        try wait { peer.drained() }
        try wait { (try? String(contentsOf: directory.appendingPathComponent("status"), encoding: .utf8)) == "closed" }
        for (actionID, reason) in refusedActions {
            try assertRefused(actionID, reason: reason, in: peer.snapshot())
        }
        let ledger = try Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor"))
        let saved = try CogletTypes.CBORCodec.decodeProfile(Array(ledger))
        guard case .map(let entries) = saved else { throw WireTestFailure() }
        XCTAssertEqual(entries.first { $0.key == "running" }?.value, .boolean(false))
        XCTAssertEqual(entries.first { $0.key == "killed" }?.value, .boolean(false))
        XCTAssertEqual(try focusOutcomes(peer.snapshot()).count, 2)
        XCTAssertLessThanOrEqual(try peer.snapshot().count, 512)
        XCTAssertLessThanOrEqual(peer.bytes().count, 1_048_576)
        let retained = XCTAttachment(data: safeAttachmentBytes(ledger), uniformTypeIdentifier: "public.data")
        retained.name = "live-app-wire-final-ledger"; retained.lifetime = .keepAlways; add(retained)
        retainedFinalLedger = true
    }
}
