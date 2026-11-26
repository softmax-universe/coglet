import XCTest
import Darwin
import CogletTypes

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
    private var invalid = false
    init(executable: String, socket: String, errors: URL) throws {
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = [socket]
        process.standardInput = input; process.standardOutput = output
        FileManager.default.createFile(atPath: errors.path, contents: nil)
        process.standardError = try FileHandle(forWritingTo: errors)
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            if data.isEmpty { handle.readabilityHandler = nil; return }
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
        let pathFile = directory.appendingPathComponent("socket-path")
        try wait { FileManager.default.fileExists(atPath: pathFile.path) }
        let socket = try String(contentsOf: pathFile, encoding: .utf8)
        XCTAssertLessThan(socket.utf8.count, 100)
        let errorFile = directory.appendingPathComponent("peer-stderr")
        let peer = try AppMockPeer(executable: executable, socket: socket, errors: errorFile)
        defer {
            let raw = XCTAttachment(data: peer.bytes(), uniformTypeIdentifier: "public.jsonl")
            raw.name = "live-app-coglet-mock-wire"; raw.lifetime = .keepAlways; add(raw)
            if let bytes = try? Data(contentsOf: errorFile) {
                let errors = XCTAttachment(data: bytes, uniformTypeIdentifier: "public.plain-text")
                errors.name = "live-app-coglet-mock-stderr"; errors.lifetime = .keepAlways; add(errors)
            }
            let tree = XCTAttachment(string: app.debugDescription)
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
                   ["conflict", "unavailable"].contains(error?["code"] as? String ?? "") { continue }
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
        // First open Settings on another real route so the following navigation
        // MUST emit view_changed, not merely an initial window_changed.
        try act("softmax.app.navigate", ["window": "settings", "route": "audio"])
        let settings = app.windows["Softmax Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5))
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
        // Captured receipts are schema-validated by the real peer; successful
        // close waits for every expected accept/outcome/replay/read response.
        let initialEpoch = try XCTUnwrap(peer.snapshot().first { $0["type"] as? String == "grants" })
        let grants = try peer.snapshot().filter { $0["type"] as? String == "grants" }
        XCTAssertEqual(grants.count, 1)
        XCTAssertGreaterThan(try XCTUnwrap(body(initialEpoch)["epoch"] as? NSNumber).uint64Value, 0)
        try peer.send(["kind": "close"])
        try wait { !peer.process.isRunning }
        XCTAssertEqual(peer.process.terminationStatus, 0)
        try wait { (try? String(contentsOf: directory.appendingPathComponent("status"), encoding: .utf8)) == "closed" }
        let ledger = try Data(contentsOf: directory.appendingPathComponent("l/ledger.cbor"))
        let saved = try CogletTypes.CBORCodec.decodeProfile(Array(ledger))
        XCTAssertEqual(saved["running"], .boolean(false))
        XCTAssertEqual(saved["killed"], .boolean(false))
        let retained = XCTAttachment(data: ledger, uniformTypeIdentifier: "public.data")
        retained.name = "live-app-wire-final-ledger"; retained.lifetime = .keepAlways; add(retained)
    }
}
