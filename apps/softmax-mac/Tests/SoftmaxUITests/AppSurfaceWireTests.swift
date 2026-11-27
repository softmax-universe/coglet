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
    private func checkboxValue(_ raw: Any?) -> Bool? {
        if let n = raw as? NSNumber, n == 0 || n == 1 { return n.boolValue }
        if let s = raw as? String, s == "0" || s == "1" { return s == "1" }
        return nil
    }

    @MainActor
    func testSoftmaxAppObserveAndControlThroughCogletMock() throws {
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
        // Mutable settings must NOT use NSArgumentDomain overrides.
        app.launchEnvironment["TMPDIR"] = temp.path
        app.launch()
        defer { app.terminate() }
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
        try wait { (try? peer.snapshot().contains { $0["type"] as? String == "grants" }) == true }
        XCTAssertTrue(peer.process.isRunning)
        // One peer only: SDK listener is closed/unlinked after authenticated accept.
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        XCTAssertGreaterThanOrEqual(fd, 0)
        if fd >= 0 {
            defer { Darwin.close(fd) }
            var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
            let bytes = Array(socket.utf8) + [0]
            withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
            let result = withUnsafePointer(to: &address) { p in
                p.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
            }
            XCTAssertEqual(result, -1)
        }
        func event(_ kind: String, matching: @escaping ([String: Any]) -> Bool) throws -> [String: Any] {
            try wait { (try? peer.snapshot().contains {
                $0["type"] as? String == "event" && self.body($0)["kind"] as? String == kind && matching(self.payload($0))
            }) == true }
            return try XCTUnwrap(peer.snapshot().last {
                $0["type"] as? String == "event" && self.body($0)["kind"] as? String == kind && matching(self.payload($0))
            })
        }
        @discardableResult
        func act(_ operation: String, _ args: [String: Any]) throws -> String {
            // Only a navigation precondition refusal may get ONE new-id retry.
            // A native press is never resubmitted under a new id.
            for attempt in 0..<2 {
                let actionID = id()
                try peer.send(["kind": "act", "operation": operation, "args": args, "id": actionID])
                func response() -> [String: Any]? {
                    try? peer.snapshot().first { frame in
                        (frame["type"] as? String == "accept" && self.body(frame)["id"] as? String == actionID
                            && self.body(frame)["accepted"] as? Bool == false)
                        || (self.body(frame)["kind"] as? String == "action.outcome"
                            && self.payload(frame)["id"] as? String == actionID)
                    }
                }
                try wait { response() != nil }
                let frame = try XCTUnwrap(response())
                if payload(frame)["status"] as? String == "succeeded" { return actionID }
                let error = (body(frame)["error"] ?? payload(frame)["error"]) as? [String: Any]
                if operation == "softmax.app.navigate", attempt == 0,
                   error?["code"] as? String == "conflict" { continue }
                XCTFail("ordinary live action was refused or failed"); throw WireTestFailure()
            }
            throw WireTestFailure()
        }
        func state() throws -> [String: Any] {
            let readID = id()
            try peer.send(["kind": "read", "operation": "softmax.app.state", "args": [:], "id": readID])
            try wait { (try? peer.snapshot().contains {
                $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
            }) == true }
            let reply = try XCTUnwrap(peer.snapshot().first {
                $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
            })
            XCTAssertEqual(body(reply)["ok"] as? Bool, true)
            return try XCTUnwrap(body(reply)["value"] as? [String: Any])
        }
        // Navigate changes an OPEN window's route; it must not implicitly open it.
        // Retain a wire-state witness for the closed-window precondition that
        // caused the original native failure, then prove the actual open path.
        let initial = try state()
        let initialWindows = try XCTUnwrap(initial["windows"] as? [[String: Any]])
        let initialSettings = try XCTUnwrap(initialWindows.first { $0["window"] as? String == "settings" })
        XCTAssertEqual(initialSettings["open"] as? Bool, false)
        try act("softmax.app.open_window", ["window": "settings"])
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        _ = try event("softmax.app.window_changed") {
            $0["window"] as? String == "settings" && $0["open"] as? Bool == true
        }
        let opened = try state()
        let openedWindows = try XCTUnwrap(opened["windows"] as? [[String: Any]])
        let openedSettings = try XCTUnwrap(openedWindows.first { $0["window"] as? String == "settings" })
        XCTAssertEqual(openedSettings["open"] as? Bool, true)
        // Visit another real route so the following navigation MUST emit
        // view_changed, not merely an initial window_changed.
        try act("softmax.app.navigate", ["window": "settings", "route": "audio"])
        XCTAssertTrue(settings.checkBoxes["settings.audio.sound_cues"].waitForExistence(timeout: 5))
        try act("softmax.app.navigate", ["window": "settings", "route": "general"])
        let navigation = try event("softmax.app.view_changed") {
            $0["window"] as? String == "settings" && $0["route"] as? String == "general"
        }
        let toggle = settings.checkBoxes["floatingOrbToggle"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 5))
        let before = try state()
        let beforeValue = try XCTUnwrap((before["settings"] as? [String: Any])?["floating_orb"] as? Bool)
        XCTAssertEqual(try XCTUnwrap(checkboxValue(toggle.value)), beforeValue)
        try act("softmax.app.set_setting", ["key": "floating_orb", "value": !beforeValue])
        try wait { self.checkboxValue(toggle.value) == !beforeValue }
        let after = try state()
        XCTAssertEqual((after["settings"] as? [String: Any])?["floating_orb"] as? Bool, !beforeValue)
        XCTAssertGreaterThanOrEqual((after["revision"] as? NSNumber)?.uint64Value ?? 0,
            (payload(navigation)["revision"] as? NSNumber)?.uint64Value ?? UInt64.max)
        try act("softmax.app.set_focus", ["window": "settings", "control": "settings.search"])
        _ = try event("softmax.app.focus_changed") {
            $0["window"] as? String == "settings" && $0["control"] as? String == "settings.search"
        }
        let search = settings.textFields["settings.search"]
        try wait { search.value(forKey: "hasKeyboardFocus") as? Bool == true }
        let focus = try XCTUnwrap(try state()["focus"] as? [String: Any])
        XCTAssertEqual(focus["control"] as? String, "settings.search")
        // Countable non-idempotent handler witness, independent of wire receipt.
        let countFile = directory.appendingPathComponent("native-press-count")
        try wait { (try? String(contentsOf: countFile, encoding: .utf8)) == "0" }
        let wrongEpochID = id()
        try peer.send(["kind": "wrong_epoch", "id": wrongEpochID])
        try wait { (try? peer.snapshot().contains {
            $0["type"] as? String == "accept" && self.body($0)["id"] as? String == wrongEpochID
                && self.body($0)["accepted"] as? Bool == false
        }) == true }
        let epochRefusal = try XCTUnwrap(peer.snapshot().first {
            $0["type"] as? String == "accept" && self.body($0)["id"] as? String == wrongEpochID
        })
        XCTAssertEqual((body(epochRefusal)["error"] as? [String: Any])?["code"] as? String, "permission_denied")
        XCTAssertEqual(try String(contentsOf: countFile, encoding: .utf8), "0")
        let pressID = try act("softmax.app.invoke", ["window": "settings", "control": "settings.shortcuts.open", "verb": "press"])
        try wait { (try? String(contentsOf: countFile, encoding: .utf8)) == "1" }
        XCTAssertTrue(app.buttons["keyboard_shortcuts.done"].waitForExistence(timeout: 5))
        let originalOutcomes = try peer.snapshot().filter {
            self.body($0)["kind"] as? String == "action.outcome" && self.payload($0)["id"] as? String == pressID
        }
        XCTAssertEqual(originalOutcomes.count, 1)
        try peer.send(["kind": "retry", "id": pressID])
        try wait { (try? peer.snapshot().filter {
            self.body($0)["kind"] as? String == "action.outcome" && self.payload($0)["id"] as? String == pressID
        }.count) == 2 }
        XCTAssertEqual(try String(contentsOf: countFile, encoding: .utf8), "1")
        // Ungranted sensitive dispatch must not enter ANY native handler.
        let deniedID = id()
        try peer.send(["kind": "act", "operation": "softmax.app.invoke_sensitive",
            "args": ["window": "settings", "control": "settings.shortcuts.open", "verb": "press"], "id": deniedID])
        try wait { (try? peer.snapshot().contains {
            $0["type"] as? String == "accept" && self.body($0)["id"] as? String == deniedID
                && self.body($0)["accepted"] as? Bool == false
        }) == true }
        XCTAssertEqual(try String(contentsOf: countFile, encoding: .utf8), "1")
        // Bounded live delta only: no reconnect/revocation/identity-token claim.
        // The closed app-control wire record has no verbs member. Derive target
        // membership/sensitivity from wire reads, then use the exact registered
        // verbs in Windows.swift / HubSettings.swift (never invent a wire field).
        func readControls(_ window: String) throws -> [[String: Any]] {
            var rows: [[String: Any]] = [], seen = Set<String>(), cursors = Set<String>()
            var after: Any = NSNull(), revision: Any = NSNull()
            for _ in 0..<32 {
                let readID = id()
                try peer.send(["kind": "read", "operation": "softmax.app.controls", "id": readID,
                    "args": ["window": window, "after": after, "limit": 128, "revision": revision]])
                try wait { (try? peer.snapshot().contains {
                    $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
                }) == true }
                let reply = try XCTUnwrap(peer.snapshot().first {
                    $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
                })
                // Stale/conflict is a failure, not a restart with mixed pages.
                guard body(reply)["ok"] as? Bool == true else {
                    XCTFail("controls page refused"); throw WireTestFailure()
                }
                let page = try XCTUnwrap(body(reply)["value"] as? [String: Any])
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
        // Sample NEW read replies outside an XCTest predicate: state()/controls
        // each wait for their own correlated response, never nested XCTWaiter calls.
        // No action retry and no swallowed read/schema/modal-owner errors. Twelve
        // attempts + a deadline bound added traffic (normally one page per read),
        // retaining the peer's hard 512-frame / 1 MiB fail-closed limits.
        var observationSamplesRemaining = 32 // shared across all readiness waits
        func observe(_ window: String, route: String, anchors: Set<String>,
                     searchText: String? = nil) throws {
            let deadline = Date().addingTimeInterval(8)
            for attempt in 0..<12 {
                guard observationSamplesRemaining > 0 else {
                    XCTFail("fresh observation sample budget exhausted"); throw WireTestFailure()
                }
                observationSamplesRemaining -= 1
                let snapshot = try state()
                let windows = try XCTUnwrap(snapshot["windows"] as? [[String: Any]])
                let ready = windows.contains { $0["window"] as? String == window
                    && $0["open"] as? Bool == true && $0["route"] as? String == route }
                if ready {
                    let rows = try readControls(window)
                    let ids = Set(rows.compactMap { $0["id"] as? String })
                    var textReady = searchText == nil
                    if let searchText, let row = rows.first(where: { $0["id"] as? String == "settings.search" }) {
                        let value = try XCTUnwrap(row["value"] as? [String: Any])
                        textReady = value["kind"] as? String == "text" && value["value"] as? String == searchText
                    }
                    if anchors.isSubset(of: ids) && textReady { return }
                }
                guard attempt < 11, Date() < deadline else { break }
                // Yield for the app's asynchronous registration/capture pump, not
                // a busy loop or an XCTest predicate invoking another waiter.
                RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.25)))
            }
            XCTFail("fresh app observation did not settle within bound"); throw WireTestFailure()
        }
        func canonical(_ value: Any) throws -> Data {
            try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        }
        func safeState() throws -> Data {
            var snapshot = try state()
            // UI revision is not itself a native effect; retain all safe state.
            snapshot.removeValue(forKey: "revision")
            return try canonical(snapshot)
        }
        var exercised: [[String: String]] = []
        func refuseSensitive(_ window: String, anchors: Set<String>) throws {
            let rows = try readControls(window)
            let protected = rows.filter { $0["sensitive"] as? Bool == true }
            let ids = Set(protected.compactMap { $0["id"] as? String })
            guard !anchors.isEmpty, anchors.isSubset(of: ids) else {
                XCTFail("required source-backed sensitive controls missing"); throw WireTestFailure()
            }
            for control in protected {
                let controlID = try XCTUnwrap(control["id"] as? String)
                let role = try XCTUnwrap(control["role"] as? String)
                let sourceButtons: Set<String> = ["settings.general.login_enable", "settings.general.login_disable",
                    "settings.general.login_settings", "settings.privacy.microphone_settings",
                    "settings.privacy.input_monitoring_settings", "settings.hub.pair",
                    "settings.hub.pairing.import_qr", "settings.hub.pairing.connect"]
                let sourceFields: Set<String> = ["settings.hub.pairing.code", "settings.hub.pairing.invitation",
                    "settings.hub.pairing.router_name", "settings.hub.pairing.router_node",
                    "settings.hub.pairing.router_addresses", "settings.hub.pairing.relay_url"]
                let verbs: [String]
                switch role {
                case "button" where sourceButtons.contains(controlID): verbs = ["press"]
                case "disclosure" where controlID == "settings.hub.pairing.manual": verbs = ["toggle"]
                case "secure_field" where sourceFields.contains(controlID): verbs = ["type_text"]
                case "text_field" where sourceFields.contains(controlID): verbs = ["type_text"]
                default:
                    XCTFail("unreviewed sensitive role/verb inventory"); throw WireTestFailure()
                }
                for verb in verbs {
                    let beforeState = try safeState()
                    let beforeControls = try canonical(readControls(window))
                    let beforeUI = canonicalAX(app.debugDescription)
                    let beforeCount = try String(contentsOf: countFile, encoding: .utf8)
                    var args: [String: Any] = ["window": window, "control": controlID, "verb": verb]
                    if verb == "toggle" { args["value"] = true }
                    if verb == "type_text" { args["value"] = "ordinary-refused-input" }
                    let actionID = id()
                    try peer.send(["kind": "act", "operation": "softmax.app.invoke", "args": args, "id": actionID])
                    func terminal() -> [String: Any]? {
                        try? peer.snapshot().first {
                            ($0["type"] as? String == "accept" && self.body($0)["id"] as? String == actionID
                                && self.body($0)["accepted"] as? Bool == false)
                            || (self.body($0)["kind"] as? String == "action.outcome"
                                && self.payload($0)["id"] as? String == actionID)
                        }
                    }
                    try wait { terminal() != nil }
                    let frame = try XCTUnwrap(terminal())
                    if frame["type"] as? String == "accept" {
                        XCTAssertEqual(body(frame)["accepted"] as? Bool, false)
                    } else {
                        XCTAssertEqual(payload(frame)["status"] as? String, "failed")
                        let accepted = try XCTUnwrap(peer.snapshot().first {
                            $0["type"] as? String == "accept" && self.body($0)["id"] as? String == actionID
                        })
                        XCTAssertEqual(body(accepted)["accepted"] as? Bool, true)
                    }
                    let error = (body(frame)["error"] ?? payload(frame)["error"]) as? [String: Any]
                    XCTAssertEqual(error?["code"] as? String, "permission_denied")
                    // Boolean comparisons keep potentially private values out of failure diagnostics.
                    XCTAssertTrue(try safeState() == beforeState, "safe state changed after refusal")
                    XCTAssertTrue(try canonical(readControls(window)) == beforeControls, "controls changed after refusal")
                    XCTAssertTrue(canonicalAX(app.debugDescription) == beforeUI, "native UI changed after refusal")
                    XCTAssertEqual(try String(contentsOf: countFile, encoding: .utf8), beforeCount)
                    XCTAssertEqual(beforeCount, "1")
                    exercised.append(["window": window, "control": controlID, "verb": verb])
                }
            }
        }
        func redacted(_ window: String, _ controlID: String) throws {
            let row = try XCTUnwrap(readControls(window).first { $0["id"] as? String == controlID })
            XCTAssertEqual(row["sensitive"] as? Bool, true)
            let value = try XCTUnwrap(row["value"] as? [String: Any])
            XCTAssertTrue(try canonical(value) == canonical(["kind": "redacted"]), "secure projection is not constant redacted")
        }
        func scanCanaries(_ additional: Data = Data()) throws {
            let sources = [peer.bytes(), try Data(contentsOf: errorFile),
                Data(app.debugDescription.utf8), additional]
            for source in sources {
                XCTAssertFalse(containsCanary(source, canaries: secretCanaries), "generated secret leaked")
            }
            for frame in try peer.snapshot() where body(frame)["kind"] as? String == "softmax.app.control_changed" {
                let p = payload(frame)
                if let key = p["control"] as? String,
                   ["settings.hub.pairing.code", "settings.hub.pairing.invitation"].contains(key) {
                    let value = try XCTUnwrap(p["value"] as? [String: Any])
                    XCTAssertTrue(try canonical(value) == canonical(["kind": "redacted"]), "secret event not redacted")
                }
            }
        }
        // Dismiss the original real shortcuts sheet before navigation.
        app.buttons["keyboard_shortcuts.done"].click()
        try wait { !app.buttons["keyboard_shortcuts.done"].exists }
        // Positive control: ordinary text is retained, not blanket-redacted.
        let publicSearch = "general"
        search.click(); search.typeText(publicSearch)
        try wait { search.value as? String == publicSearch }
        try observe("settings", route: "general", anchors: ["settings.search"], searchText: publicSearch)
        let searchRow = try XCTUnwrap(readControls("settings").first { $0["id"] as? String == "settings.search" })
        let searchValue = try XCTUnwrap(searchRow["value"] as? [String: Any])
        XCTAssertEqual(searchValue["kind"] as? String, "text")
        XCTAssertEqual(searchValue["value"] as? String, publicSearch)
        search.typeKey("a", modifierFlags: .command); search.typeKey(.delete, modifierFlags: [])
        try observe("settings", route: "general", anchors: ["settings.search"], searchText: "")
        for (route, anchors) in [
            ("general", Set(["settings.general.login_enable", "settings.general.login_disable", "settings.general.login_settings"])),
            ("privacy", Set(["settings.privacy.microphone_settings", "settings.privacy.input_monitoring_settings"])),
            ("hub", Set(["settings.hub.pair"]))
        ] {
            try act("softmax.app.navigate", ["window": "settings", "route": route])
            XCTAssertTrue(settings.buttons[anchors.sorted()[0]].waitForExistence(timeout: 5))
            try observe("settings", route: route, anchors: anchors)
            try refuseSensitive("settings", anchors: anchors)
        }
        // Native USER click only. configureHub() returns immediately when testing;
        // HubSettings still presents its actual app-owned sheet. No sensitive grant.
        settings.buttons["settings.hub.pair"].click()
        let codeField = app.secureTextFields["settings.hub.pairing.code"]
        XCTAssertTrue(codeField.waitForExistence(timeout: 5))
        try observe("hub_pairing", route: "code", anchors: ["settings.hub.pairing.code",
            "settings.hub.pairing.manual", "settings.hub.pairing.import_qr", "settings.hub.pairing.connect"])
        let sheetState = try state() // appSnapshot modal-owner guard must recognize this sheet.
        let sheetWindows = try XCTUnwrap(sheetState["windows"] as? [[String: Any]])
        XCTAssertTrue(sheetWindows.contains { $0["window"] as? String == "hub_pairing"
            && $0["open"] as? Bool == true && $0["route"] as? String == "code" })
        try redacted("hub_pairing", "settings.hub.pairing.code")
        try refuseSensitive("hub_pairing", anchors: ["settings.hub.pairing.code", "settings.hub.pairing.manual",
            "settings.hub.pairing.import_qr", "settings.hub.pairing.connect"])
        // ADR-0030 private input/audit boundary: XCTest typeText activities (and
        // xcresult/xcodebuild input logs) are NOT scanned here. This bounded
        // observation/event/ledger proof is NOT total diagnostic secrecy or full
        // ADR acceptance; private input retention/audit requires a separate gate.
        let emptyCodeUI = codeField.value as? String
        codeField.click(); codeField.typeText(secretCanaries[0])
        try wait { (codeField.value as? String) != emptyCodeUI }
        try wait { codeField.value(forKey: "hasKeyboardFocus") as? Bool == true }
        _ = try event("softmax.app.focus_changed") { $0["control"] as? String == "settings.hub.pairing.code" }
        try observe("hub_pairing", route: "code", anchors: ["settings.hub.pairing.code"])
        try redacted("hub_pairing", "settings.hub.pairing.code")
        try scanCanaries()
        // Disclosure is expanded through its real native user control, NOT an
        // adapter sensitive invocation, and neither Connect nor QR is clicked.
        // HubSettings attaches the exact ID to the DisclosureGroup: use its
        // native triangle, never a whole-group centre or an unrelated triangle.
        let manualID = "settings.hub.pairing.manual"
        let manual = app.descendants(matching: .any).matching(identifier: manualID).firstMatch
        XCTAssertTrue(manual.exists)
        let identifiedTriangle = app.disclosureTriangles[manualID]
        let triangle = identifiedTriangle.exists ? identifiedTriangle : manual.disclosureTriangles.firstMatch
        guard triangle.exists else {
            XCTFail("owned manual disclosure triangle missing"); throw WireTestFailure()
        }
        // The macOS AX DisclosureTriangle frame includes its label: the
        // default centre click hits text, not the native chevron. The retained
        // bbab qualification records frame x=264,width=252.5 and chevron x=290.
        // Keep the click relative to this identified element (26 points in),
        // never an adapter toggle or a whole-group/label-centre click.
        let triangleFrame = triangle.frame
        guard triangleFrame.width > 52, triangleFrame.height > 0,
              triangleFrame.width.isFinite, triangleFrame.height.isFinite else {
            XCTFail("owned manual disclosure geometry unavailable"); throw WireTestFailure()
        }
        XCTAssertTrue((triangle.value as? String) == "0" || (triangle.value as? NSNumber)?.intValue == 0)
        triangle.coordinate(withNormalizedOffset: CGVector(dx: 26 / triangleFrame.width, dy: 0.5)).click()
        try wait { (triangle.value as? String) == "1" || (triangle.value as? NSNumber)?.intValue == 1 }
        // Native census regression: a container identifier must not replace
        // the independently registered identities of its expanded inputs.
        for fieldID in ["settings.hub.pairing.router_name", "settings.hub.pairing.router_node",
                        "settings.hub.pairing.router_addresses", "settings.hub.pairing.relay_url"] {
            let fields = app.textFields.matching(identifier: fieldID)
            XCTAssertTrue(fields.firstMatch.waitForExistence(timeout: 5), "missing native field: \(fieldID)")
            XCTAssertEqual(fields.count, 1, "ambiguous native field: \(fieldID)")
        }
        XCTAssertEqual(app.textFields.matching(identifier: manualID).count, 0)
        XCTAssertEqual(app.secureTextFields.matching(identifier: manualID).count, 0)
        let invitation = app.secureTextFields["settings.hub.pairing.invitation"]
        XCTAssertTrue(invitation.waitForExistence(timeout: 5))
        XCTAssertEqual(app.secureTextFields.matching(identifier: "settings.hub.pairing.invitation").count, 1)
        try observe("hub_pairing", route: "manual", anchors: ["settings.hub.pairing.invitation",
            "settings.hub.pairing.router_name", "settings.hub.pairing.router_node",
            "settings.hub.pairing.router_addresses", "settings.hub.pairing.relay_url"])
        try redacted("hub_pairing", "settings.hub.pairing.invitation")
        let emptyInvitationUI = invitation.value as? String
        // Exact 15e6915 native evidence: invitation y=503...530 is below
        // its viewport y=187...499; click() targets the blank fixed footer.
        // Reveal through this field's unique owning native sheet/scroll view,
        // not an adapter action, alternate selector, or click retry.
        let invitationID = "settings.hub.pairing.invitation"
        let pairingSheets = app.sheets.containing(.secureTextField, identifier: invitationID)
        guard pairingSheets.count == 1 else { XCTFail("invitation sheet ambiguous"); throw WireTestFailure() }
        let pairingScrolls = pairingSheets.firstMatch.scrollViews.containing(.secureTextField, identifier: invitationID)
        guard pairingScrolls.count == 1 else { XCTFail("invitation viewport ambiguous"); throw WireTestFailure() }
        let pairingScroll = pairingScrolls.firstMatch
        pairingScroll.scroll(byDeltaX: 0, deltaY: -200)
        var revealGeometry = "not sampled"
        do {
            try wait {
                let field = invitation.frame, viewport = pairingScroll.frame
                // Sample hit testing independently, even if geometry is false.
                let hittable = invitation.isHittable
                let finite = [field.minX, field.minY, field.width, field.height,
                    viewport.minX, viewport.minY, viewport.width, viewport.height].allSatisfy({ $0.isFinite })
                let positive = field.width > 0 && field.height > 0 && viewport.width > 0 && viewport.height > 0
                // Exact 4a275d98 AX: content is at its end, field y473...500,
                // viewport y187...499. Permit only the measured 1pt border,
                // never an offscreen centre (the earlier y503...530 failure).
                let vertical = field.minY >= viewport.minY && field.maxY <= viewport.maxY + 1
                let centre = field.midY > viewport.minY && field.midY < viewport.maxY
                    && field.midX > viewport.minX && field.midX < viewport.maxX
                revealGeometry = "field=\(field); viewport=\(viewport); finite=\(finite); positive=\(positive); vertical=\(vertical); centre=\(centre); hittable=\(hittable)"
                return finite && positive && vertical && centre && hittable
            }
        } catch {
            let geometry = XCTAttachment(string: revealGeometry)
            geometry.name = "live-app-wire-invitation-reveal-geometry"; geometry.lifetime = .keepAlways; add(geometry)
            throw error
        }
        invitation.click()
        try wait { invitation.value(forKey: "hasKeyboardFocus") as? Bool == true }
        invitation.typeText(secretCanaries[1])
        try wait { (invitation.value as? String) != emptyInvitationUI }
        try wait { invitation.value(forKey: "hasKeyboardFocus") as? Bool == true }
        _ = try event("softmax.app.focus_changed") { $0["control"] as? String == "settings.hub.pairing.invitation" }
        try observe("hub_pairing", route: "manual", anchors: ["settings.hub.pairing.code", "settings.hub.pairing.invitation"])
        try redacted("hub_pairing", "settings.hub.pairing.code")
        try redacted("hub_pairing", "settings.hub.pairing.invitation")
        try refuseSensitive("hub_pairing", anchors: ["settings.hub.pairing.invitation", "settings.hub.pairing.router_name",
            "settings.hub.pairing.router_node", "settings.hub.pairing.router_addresses", "settings.hub.pairing.relay_url"])
        try scanCanaries()
        app.buttons["settings.hub.pairing.cancel"].click()
        try wait { !app.buttons["settings.hub.pairing.cancel"].exists }
        try scanCanaries()
        let nativePressCount = try String(contentsOf: countFile, encoding: .utf8)
        XCTAssertEqual(nativePressCount, "1")
        let evidence = try canonical(["scope": "bounded live routes and pairing fields only",
            "excluded_private_input_audit": "XCTest typeText activities, xcresult and xcodebuild input logs not scanned; not total diagnostic secrecy or full ADR acceptance",
            "native_press_count": nativePressCount, "exercised": exercised] as [String: Any])
        let bounded = XCTAttachment(data: evidence, uniformTypeIdentifier: "public.json")
        bounded.name = "live-app-wire-bounded-sensitive-evidence"; bounded.lifetime = .keepAlways; add(bounded)
        // Captured receipts are schema-validated by the real peer; successful
        // close waits for every expected accept/outcome/replay/read response.
        let initialEpoch = try XCTUnwrap(peer.snapshot().first { $0["type"] as? String == "grants" })
        let grants = try peer.snapshot().filter { $0["type"] as? String == "grants" }
        XCTAssertEqual(grants.count, 1)
        XCTAssertGreaterThan(try XCTUnwrap(body(initialEpoch)["epoch"] as? NSNumber).uint64Value, 0)
        try peer.send(["kind": "close"])
        try wait { !peer.process.isRunning }
        XCTAssertEqual(peer.process.terminationStatus, 0)
        try wait { peer.drained() }
        try wait { (try? String(contentsOf: directory.appendingPathComponent("status"), encoding: .utf8)) == "closed" }
        let ledger = try Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor"))
        let saved = try CogletTypes.CBORCodec.decodeProfile(Array(ledger))
        guard case .map(let entries) = saved else { throw WireTestFailure() }
        XCTAssertEqual(entries.first { $0.key == "running" }?.value, .boolean(false))
        XCTAssertEqual(entries.first { $0.key == "killed" }?.value, .boolean(false))
        // Include bytes produced during close and the FINAL durable ledger.
        try scanCanaries(ledger)
        let retained = XCTAttachment(data: safeAttachmentBytes(ledger), uniformTypeIdentifier: "public.data")
        retained.name = "live-app-wire-final-ledger"; retained.lifetime = .keepAlways; add(retained)
        retainedFinalLedger = true
    }
}

// MARK: - Actual-account credential secrecy (separate from the original proof)
//
// Same-file extension: the original test body above is untouched and the
// private AppMockPeer/WireTestFailure/helpers remain private to this file.
extension AppSurfaceWireTests {
    private func credentialHexData(_ hex: String) -> Data {
        var result = Data()
        var index = hex.startIndex
        while index < hex.endIndex, let next = hex.index(index, offsetBy: 2, limitedBy: hex.endIndex) {
            result.append(UInt8(hex[index..<next], radix: 16) ?? 0)
            index = next
        }
        return result
    }
    private func credentialLowerHex(_ data: Data) -> String { data.map { String(format: "%02x", $0) }.joined() }
    private func credentialURLSafe(_ text: String) -> String {
        text.replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
    }
    /// Alignment-robust base64 fragments: a payload embedded after 0/1/2 bytes
    /// in a larger stream encodes differently, so keep only characters fully
    /// determined by the payload (drop prefix-mixed and suffix-mixed characters).
    private func credentialBase64Fragments(_ payload: Data) -> [Data] {
        var result: [Data] = []
        for pad in 0..<3 {
            let encoded = (Data(repeating: 0, count: pad) + payload).base64EncodedString()
            let body = encoded.replacingOccurrences(of: "=", with: "")
            let drop = [0, 2, 3][pad]
            let tail = (payload.count + pad) % 3 == 0 ? 0 : 1
            guard body.count > drop + tail + 16 else { continue }
            let fragment = String(body.dropFirst(drop).dropLast(tail))
            result.append(Data(fragment.utf8))
            result.append(Data(credentialURLSafe(fragment).utf8))
        }
        return result
    }
    /// Raw bytes, lowercase/uppercase hex, and every base64/base64url alignment,
    /// for both the decoded 32 bytes and the token's own UTF-8 text.
    private func credentialNeedles(_ secrets: [String]) -> [Data] {
        var needles: [Data] = []
        for secret in secrets {
            for payload in [credentialHexData(secret), Data(secret.utf8)] {
                needles.append(payload)
                needles.append(Data(credentialLowerHex(payload).utf8))
                needles.append(Data(credentialLowerHex(payload).uppercased().utf8))
                needles.append(contentsOf: credentialBase64Fragments(payload))
            }
            needles.append(Data(secret.uppercased().utf8))
        }
        var seen = Set<Data>()
        return needles.filter { !$0.isEmpty && seen.insert($0).inserted }
    }
    private func credentialLeak(_ bytes: Data, needles: [Data]) -> Bool {
        needles.contains { bytes.range(of: $0) != nil }
    }
    private func credentialIsBool(_ raw: Any?, _ expected: Bool) -> Bool {
        guard let number = raw as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else { return false }
        return number.boolValue == expected
    }
    /// Closed native fixture witness: Booleans/counters/enum names only.
    private func credentialWitness(_ directory: URL, stopped: Bool) -> Bool {
        guard let data = try? Data(contentsOf: directory.appendingPathComponent("credential-witness")),
              data.count <= 512,
              let value = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              Set(value.keys) == ["seeded", "key_started", "token_connected", "installs", "state", "stopped", "link_stops"]
        else { return false }
        return credentialIsBool(value["seeded"], true) && credentialIsBool(value["key_started"], true)
            && credentialIsBool(value["token_connected"], true) && credentialIsBool(value["stopped"], stopped)
            && (value["installs"] as? NSNumber)?.intValue == 1
            && value["state"] as? String == (stopped ? "offline" : "connecting")
            && (value["link_stops"] as? NSNumber)?.intValue == (stopped ? 1 : 0)
    }

    func testCredentialScannerDetectsEveryEncodingAndAlignment() {
        let secrets = ["00112233445566778899aabbccddeeff0123456789abcdef0011223344556677",
                       "fedcba9876543210ffeeddccbbaa99887766554433221100abcdef0123456789"]
        let needles = credentialNeedles(secrets)
        for secret in secrets {
            for payload in [credentialHexData(secret), Data(secret.utf8)] {
                for prefix in 0..<3 {
                    for suffix in 0..<3 {
                        let blob = Data(repeating: 0x5a, count: prefix) + payload + Data(repeating: 0xa5, count: suffix)
                        let standard = blob.base64EncodedString()
                        let encodings: [(String, Data)] = [
                            ("raw", blob), ("base64", Data(standard.utf8)),
                            ("base64url", Data(credentialURLSafe(standard).utf8)),
                            ("unpadded", Data(standard.replacingOccurrences(of: "=", with: "").utf8)),
                            ("lower hex", Data(credentialLowerHex(blob).utf8)),
                            ("upper hex", Data(credentialLowerHex(blob).uppercased().utf8))]
                        for (name, encoded) in encodings {
                            XCTAssertTrue(credentialLeak(Data("noise".utf8) + encoded + Data("tail".utf8), needles: needles),
                                "scanner missed \(name) prefix \(prefix) suffix \(suffix)")
                        }
                    }
                }
            }
        }
        // Negative controls: unrelated bytes and a one-byte mutation do not match.
        XCTAssertFalse(credentialLeak(Data("unrelated evidence with state connecting".utf8), needles: needles))
        var mutated = credentialHexData(secrets[0]); mutated[5] ^= 0x01
        XCTAssertFalse(credentialLeak(mutated, needles: needles))
        XCTAssertFalse(credentialLeak(Data(credentialLowerHex(mutated).utf8), needles: needles))
        XCTAssertFalse(credentialLeak(Data(mutated.base64EncodedString().utf8), needles: needles))
    }

    @MainActor
    func testActualAccountCredentialsStayOutOfLiveWireUIAndEvidence() throws {
        let executable = try XCTUnwrap(ProcessInfo.processInfo.environment["SOFTMAX_APP_MOCK_PEER"])
        XCTAssertTrue(FileManager.default.isExecutableFile(atPath: executable))
        let temp = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
        let directory = temp.appendingPathComponent("sc-" + String(UUID().uuidString.prefix(8)))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        // Synthetic per-run canaries only: 32-byte identity hex and a valid invite hex.
        let identity = id() + id(), invite = id() + id()
        XCTAssertNotEqual(identity, invite)
        let secrets = [identity, invite]
        let needles = credentialNeedles(secrets)
        // Scanner positive controls on THESE canaries before any evidence is trusted.
        for secret in secrets {
            XCTAssertTrue(credentialLeak(Data("x".utf8) + Data(secret.utf8), needles: needles))
            XCTAssertTrue(credentialLeak(Data(credentialHexData(secret).base64EncodedString().utf8), needles: needles))
            XCTAssertTrue(credentialLeak(Data(secret.uppercased().utf8), needles: needles))
        }
        XCTAssertFalse(credentialLeak(Data("Router connecting".utf8), needles: needles))
        let credentialFile = directory.appendingPathComponent("credentials.json")
        let fixture = try JSONSerialization.data(withJSONObject: ["identity": identity, "invite": invite],
            options: [.sortedKeys])
        XCTAssertTrue(FileManager.default.createFile(atPath: credentialFile.path, contents: fixture,
            attributes: [.posixPermissions: 0o600]))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: credentialFile.path)
        let app = XCUIApplication()
        // Distinct credential flag, plus the isolated UI-test and private-directory flags.
        app.launchArguments = ["--ui-testing", "--sdk-wire-proof-directory", directory.path,
            "--sdk-wire-proof-credentials"]
        app.launchEnvironment["TMPDIR"] = temp.path
        app.launch()
        defer { app.terminate() }
        var retainedFinalLedger = false
        func safeAttachmentBytes(_ bytes: Data) -> Data {
            let leaked = credentialLeak(bytes, needles: needles)
            XCTAssertFalse(leaked, "generated credential leaked into evidence")
            return leaked ? Data("Evidence withheld: generated credential detected.".utf8) : bytes
        }
        // Observe-only failure evidence; never the original fixture inputs.
        defer {
            if let bytes = try? Data(contentsOf: directory.appendingPathComponent("failure.json")) {
                let safe = safeAttachmentBytes(bytes)
                let valid = closedDiagnostic(safe)
                XCTAssertTrue(valid, "wire failure diagnostic must have a closed schema")
                let attachment = XCTAttachment(data: valid ? safe : Data("Diagnostic withheld: invalid schema.".utf8),
                    uniformTypeIdentifier: "public.json")
                attachment.name = "live-app-credential-failure-diagnostic"; attachment.lifetime = .keepAlways; add(attachment)
            }
            if let bytes = try? Data(contentsOf: directory.appendingPathComponent("status")),
               bytes == Data("active".utf8) || bytes == Data("closed".utf8) || bytes == Data("close_failed".utf8) {
                let attachment = XCTAttachment(data: safeAttachmentBytes(bytes), uniformTypeIdentifier: "public.plain-text")
                attachment.name = "live-app-credential-terminal-status"; attachment.lifetime = .keepAlways; add(attachment)
                if !retainedFinalLedger, bytes == Data("closed".utf8),
                   let ledger = try? Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor")),
                   let saved = try? CogletTypes.CBORCodec.decodeProfile(Array(ledger)),
                   case .map(let entries) = saved,
                   entries.first(where: { $0.key == "running" })?.value == .boolean(false),
                   entries.first(where: { $0.key == "killed" })?.value == .boolean(false) {
                    let retained = XCTAttachment(data: safeAttachmentBytes(ledger), uniformTypeIdentifier: "public.data")
                    retained.name = "live-app-credential-failure-closed-ledger"; retained.lifetime = .keepAlways; add(retained)
                }
            }
            if let bytes = try? Data(contentsOf: directory.appendingPathComponent("credential-witness")), bytes.count <= 512 {
                let witness = XCTAttachment(data: safeAttachmentBytes(bytes), uniformTypeIdentifier: "public.json")
                witness.name = "live-app-credential-native-witness"; witness.lifetime = .keepAlways; add(witness)
            }
        }
        let pathFile = directory.appendingPathComponent("socket-path")
        try wait { FileManager.default.fileExists(atPath: pathFile.path) }
        let socket = try String(contentsOf: pathFile, encoding: .utf8)
        XCTAssertLessThan(socket.utf8.count, 100)
        let errorFile = directory.appendingPathComponent("peer-stderr")
        let peer = try AppMockPeer(executable: executable, socket: socket, errors: errorFile)
        defer {
            // Raw transcript, stderr and AX; the credential fixture file is never attached.
            let raw = XCTAttachment(data: safeAttachmentBytes(peer.bytes()), uniformTypeIdentifier: "public.jsonl")
            raw.name = "live-app-credential-coglet-mock-wire"; raw.lifetime = .keepAlways; add(raw)
            if let bytes = try? Data(contentsOf: errorFile) {
                let errors = XCTAttachment(data: safeAttachmentBytes(bytes), uniformTypeIdentifier: "public.plain-text")
                errors.name = "live-app-credential-coglet-mock-stderr"; errors.lifetime = .keepAlways; add(errors)
            }
            let tree = XCTAttachment(data: safeAttachmentBytes(Data(app.debugDescription.utf8)), uniformTypeIdentifier: "public.plain-text")
            tree.name = "live-app-credential-final-ax"; tree.lifetime = .keepAlways; add(tree)
            peer.terminate()
        }
        try wait { (try? peer.snapshot().contains { $0["type"] as? String == "grants" }) == true }
        XCTAssertTrue(peer.process.isRunning)
        // Native fixture seeded after directory validation and before the first baseline.
        try wait { self.credentialWitness(directory, stopped: false) }

        func event(_ kind: String, matching: @escaping ([String: Any]) -> Bool) throws -> [String: Any] {
            try wait { (try? peer.snapshot().contains {
                $0["type"] as? String == "event" && self.body($0)["kind"] as? String == kind && matching(self.payload($0))
            }) == true }
            return try XCTUnwrap(peer.snapshot().last {
                $0["type"] as? String == "event" && self.body($0)["kind"] as? String == kind && matching(self.payload($0))
            })
        }
        @discardableResult
        func act(_ operation: String, _ args: [String: Any]) throws -> String {
            let actionID = id()
            try peer.send(["kind": "act", "operation": operation, "args": args, "id": actionID])
            func response() -> [String: Any]? {
                try? peer.snapshot().first { frame in
                    (frame["type"] as? String == "accept" && self.body(frame)["id"] as? String == actionID
                        && self.body(frame)["accepted"] as? Bool == false)
                    || (self.body(frame)["kind"] as? String == "action.outcome"
                        && self.payload(frame)["id"] as? String == actionID)
                }
            }
            try wait { response() != nil }
            let frame = try XCTUnwrap(response())
            guard payload(frame)["status"] as? String == "succeeded" else {
                XCTFail("ordinary live action was refused or failed"); throw WireTestFailure()
            }
            return actionID
        }
        var stateSamplesRemaining = 24 // bounded added traffic; peer keeps 512 frames / 1 MiB
        func state() throws -> [String: Any] {
            guard stateSamplesRemaining > 0 else { XCTFail("state sample budget exhausted"); throw WireTestFailure() }
            stateSamplesRemaining -= 1
            let readID = id()
            try peer.send(["kind": "read", "operation": "softmax.app.state", "args": [:], "id": readID])
            try wait { (try? peer.snapshot().contains {
                $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
            }) == true }
            let reply = try XCTUnwrap(peer.snapshot().first {
                $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
            })
            XCTAssertEqual(body(reply)["ok"] as? Bool, true)
            return try XCTUnwrap(body(reply)["value"] as? [String: Any])
        }
        func readControls(_ window: String) throws -> [[String: Any]] {
            var rows: [[String: Any]] = [], seen = Set<String>(), cursors = Set<String>()
            var after: Any = NSNull(), revision: Any = NSNull()
            for _ in 0..<32 {
                let readID = id()
                try peer.send(["kind": "read", "operation": "softmax.app.controls", "id": readID,
                    "args": ["window": window, "after": after, "limit": 128, "revision": revision]])
                try wait { (try? peer.snapshot().contains {
                    $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
                }) == true }
                let reply = try XCTUnwrap(peer.snapshot().first {
                    $0["type"] as? String == "read_reply" && self.body($0)["correlation_id"] as? String == readID
                })
                guard body(reply)["ok"] as? Bool == true else { XCTFail("controls page refused"); throw WireTestFailure() }
                let page = try XCTUnwrap(body(reply)["value"] as? [String: Any])
                XCTAssertEqual(page["window"] as? String, window)
                let current = try XCTUnwrap(page["revision"] as? NSNumber)
                if let expected = revision as? NSNumber { XCTAssertEqual(current, expected) }
                revision = current
                let batch = try XCTUnwrap(page["controls"] as? [[String: Any]])
                XCTAssertLessThanOrEqual(batch.count, 128)
                for row in batch {
                    let key = try XCTUnwrap(row["id"] as? String)
                    guard seen.insert(key).inserted else { XCTFail("duplicate wire control"); throw WireTestFailure() }
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
        // Fresh sampling outside XCTest predicates; no retry of an action, no filtering
        // of dynamically registered extra controls (a native defect must stay RED).
        func observe(_ window: String, route: String, anchors: Set<String>) throws -> [[String: Any]] {
            let deadline = Date().addingTimeInterval(8)
            for attempt in 0..<12 {
                let snapshot = try state()
                let windows = try XCTUnwrap(snapshot["windows"] as? [[String: Any]])
                if windows.contains(where: { $0["window"] as? String == window
                    && $0["open"] as? Bool == true && $0["route"] as? String == route }) {
                    let rows = try readControls(window)
                    if anchors.isSubset(of: Set(rows.compactMap { $0["id"] as? String })) { return rows }
                }
                guard attempt < 11, Date() < deadline else { break }
                RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.25)))
            }
            XCTFail("fresh app observation did not settle within bound"); throw WireTestFailure()
        }
        func scan(_ additional: Data = Data()) throws {
            var sources = [peer.bytes(), try Data(contentsOf: errorFile), Data(app.debugDescription.utf8), additional]
            for name in ["credential-witness", "status", "failure.json"] {
                if let bytes = try? Data(contentsOf: directory.appendingPathComponent(name)) { sources.append(bytes) }
            }
            for source in sources {
                XCTAssertFalse(credentialLeak(source, needles: needles), "generated credential leaked")
            }
        }
        func requireRow(_ rows: [[String: Any]], _ controlID: String) throws -> [String: Any] {
            try XCTUnwrap(rows.first { $0["id"] as? String == controlID })
        }

        // 1. Wire state: the seeded REAL HubConnection projects only the enum.
        let initial = try state()
        XCTAssertEqual(initial["hub"] as? String, "connecting")
        XCTAssertTrue(credentialWitness(directory, stopped: false), "native fixture witness changed before use")
        try scan()
        // 2. Open Settings (general) and prove ordinary control + event positive path.
        let initialWindows = try XCTUnwrap(initial["windows"] as? [[String: Any]])
        XCTAssertEqual(initialWindows.first { $0["window"] as? String == "settings" }?["open"] as? Bool, false)
        try act("softmax.app.open_window", ["window": "settings"])
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
        _ = try event("softmax.app.window_changed") { $0["window"] as? String == "settings" && $0["open"] as? Bool == true }
        _ = try observe("settings", route: "general", anchors: [])
        let toggle = settings.checkBoxes["floatingOrbToggle"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 5))
        let before = try state()
        let beforeValue = try XCTUnwrap((before["settings"] as? [String: Any])?["floating_orb"] as? Bool)
        XCTAssertEqual(try XCTUnwrap(checkboxValue(toggle.value)), beforeValue)
        try act("softmax.app.set_setting", ["key": "floating_orb", "value": !beforeValue])
        try wait { self.checkboxValue(toggle.value) == !beforeValue }
        _ = try event("softmax.app.control_changed") {
            $0["window"] as? String == "settings" && $0["control"] as? String == "floatingOrbToggle"
        }
        let afterToggle = try state()
        XCTAssertEqual((afterToggle["settings"] as? [String: Any])?["floating_orb"] as? Bool, !beforeValue)
        XCTAssertEqual(afterToggle["hub"] as? String, "connecting")
        try scan()
        // 3. Actual Hub settings route: independent native UI text for the seeded account.
        try act("softmax.app.navigate", ["window": "settings", "route": "hub"])
        _ = try event("softmax.app.view_changed") { $0["window"] as? String == "settings" && $0["route"] as? String == "hub" }
        XCTAssertTrue(app.staticTexts["Router connecting\u{2026}"].waitForExistence(timeout: 5),
            "native UI does not show the connecting router state")
        XCTAssertTrue(settings.buttons["settings.hub.sign_out"].waitForExistence(timeout: 5))
        let pair = settings.buttons["settings.hub.pair"]
        XCTAssertTrue(pair.waitForExistence(timeout: 5))
        XCTAssertFalse(pair.isEnabled, "pairing must be disabled while enrollment is connecting")
        let hubRows = try observe("settings", route: "hub", anchors: ["settings.hub.device_identity", "settings.hub.sign_out"])
        let disclosure = try XCTUnwrap(try requireRow(hubRows, "settings.hub.device_identity")["value"] as? [String: Any])
        XCTAssertEqual(disclosure["kind"] as? String, "boolean")
        XCTAssertEqual(disclosure["value"] as? Bool, false)
        XCTAssertEqual(try state()["hub"] as? String, "connecting")
        try scan()
        // 4. Expand the public Device identity disclosure through the granted adapter
        // control (not sensitive). Public device ID is visible; secrets must not be.
        try act("softmax.app.invoke", ["window": "settings", "control": "settings.hub.device_identity",
            "verb": "toggle", "value": true])
        XCTAssertTrue(app.staticTexts[String(repeating: "d", count: 64)].waitForExistence(timeout: 5),
            "public device identifier not shown by native UI")
        let expandedRows = try observe("settings", route: "hub", anchors: ["settings.hub.device_identity",
            "settings.hub.copy_device_id", "settings.hub.sign_out"])
        let expanded = try XCTUnwrap(try requireRow(expandedRows, "settings.hub.device_identity")["value"] as? [String: Any])
        XCTAssertEqual(expanded["value"] as? Bool, true)
        XCTAssertTrue(app.descendants(matching: .any).matching(identifier: "settings.hub.copy_device_id").firstMatch
            .waitForExistence(timeout: 5), "copy device control missing from native AX")
        XCTAssertEqual(settings.disclosureTriangles.matching(identifier: "settings.hub.device_identity").count, 1,
            "device disclosure must have one native header identity")
        XCTAssertEqual(settings.buttons.matching(identifier: "settings.hub.copy_device_id").count, 1,
            "copy device button must keep its own unique native identity")
        XCTAssertEqual(settings.buttons.matching(identifier: "settings.hub.device_identity").count, 0,
            "disclosure identity propagated to a child button")
        XCTAssertEqual(settings.staticTexts.matching(identifier: "settings.hub.device_identity").count, 0,
            "disclosure identity propagated to public device text")
        try scan()
        // 5. Clean shutdown: peer 0 + orderly close, fixture stop witness, closed ledger.
        let grants = try peer.snapshot().filter { $0["type"] as? String == "grants" }
        XCTAssertEqual(grants.count, 1)
        let frameCount = try peer.snapshot().count
        XCTAssertLessThanOrEqual(frameCount, 512)
        XCTAssertLessThanOrEqual(peer.bytes().count, 1_048_576)
        try peer.send(["kind": "close"])
        try wait { !peer.process.isRunning }
        XCTAssertEqual(peer.process.terminationStatus, 0)
        try wait { peer.drained() }
        try wait { (try? String(contentsOf: directory.appendingPathComponent("status"), encoding: .utf8)) == "closed" }
        XCTAssertTrue(credentialWitness(directory, stopped: true), "native fixture did not stop with the proof")
        let ledger = try Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor"))
        let saved = try CogletTypes.CBORCodec.decodeProfile(Array(ledger))
        guard case .map(let entries) = saved else { throw WireTestFailure() }
        XCTAssertEqual(entries.first { $0.key == "running" }?.value, .boolean(false))
        XCTAssertEqual(entries.first { $0.key == "killed" }?.value, .boolean(false))
        try scan(ledger)
        let retained = XCTAttachment(data: safeAttachmentBytes(ledger), uniformTypeIdentifier: "public.data")
        retained.name = "live-app-credential-final-ledger"; retained.lifetime = .keepAlways; add(retained)
        retainedFinalLedger = true
        let summary: [String: Any] = [
            "scope": "synthetic identity/invite through production HubAccount+HubConnection with in-memory fake store/link; bounded live routes only",
            "excluded": "real Keychain, ProcessHubLink helper stdin, network, native process reconnect, partial writer, XCTest input logs; not full ADR acceptance",
            "hub_wire_state": "connecting", "peer_exit": Int(peer.process.terminationStatus), "frames": frameCount,
            "needle_count": needles.count, "ordinary_event": "control_changed floatingOrbToggle"]
        let closed = XCTAttachment(data: try JSONSerialization.data(withJSONObject: summary, options: [.sortedKeys]),
            uniformTypeIdentifier: "public.json")
        closed.name = "live-app-credential-closed-summary"; closed.lifetime = .keepAlways; add(closed)
    }
}
