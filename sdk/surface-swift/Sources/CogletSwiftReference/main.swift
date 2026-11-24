import Foundation
import CogletTypes
import CogletSurface
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

// A test-only semantic renderer/config backend. No microphone, provider or
// physical output is advertised as working by a successful fabricated receipt.
// Optional native routes are unavailable until a product supplies a real backend.
final class ReferenceBackend: SurfaceNativeBackend {
    let speechConfig: CBORValue
    var state: CBORValue
    var expires: UInt64 = 0
    init(config: CBORValue) { speechConfig = config["speech_config"]!; state = config["initial_expression"]! }
    func preflight(action: CBORValue, now: UInt64) throws {
        switch action["capability"] {
        case .text("expression.set_expression"): break
        case .text("speech.stop"): throw NativeSurfaceError("not_found")
        default: throw NativeSurfaceError("unavailable")
        }
    }
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        let args = action["args"]!
        guard case .unsigned(let duration) = args["duration_ms"] else { throw NativeSurfaceError("invalid_argument") }
        if duration == 0 { stop(owner: nil) }
        else {
            guard now <= UInt64.max - duration * 1000 else { throw NativeSurfaceError("invalid_argument") }
            expires = now + duration * 1000
            state = .record([("expression", args["expression"]!), ("intensity", args["intensity"]!), ("remaining_ms", .unsigned(duration))])
        }
        return .succeeded(.map([]))
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        switch operation {
        case "speech.config": return speechConfig
        case "expression.state":
            if now >= expires { stop(owner: nil) }
            return .record([("expression", state["expression"]!), ("intensity", state["intensity"]!),
                ("remaining_ms", .unsigned(expires > now ? (expires - now + 999) / 1000 : 0))])
        default: throw NativeSurfaceError("unavailable")
        }
    }
    func stop(owner: String?) {
        expires = 0
        state = .record([("expression", .text("neutral")), ("intensity", .unsigned(0)), ("remaining_ms", .unsigned(0))])
    }
    func tick(now: UInt64) { if now >= expires { stop(owner: nil) } }
}

// Fixture JSON only. Decode integer/boolean domains without NSNumber coercion.
indirect enum FixtureValue: Decodable {
    case value(CBORValue)
    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .value(.null) }
        else if let boolean = try? container.decode(Bool.self) { self = .value(.boolean(boolean)) }
        else if let number = try? container.decode(UInt64.self) { self = .value(.unsigned(number)) }
        else if let number = try? container.decode(Int64.self), number < 0 { self = .value(.negative(number)) }
        else if let text = try? container.decode(String.self) { self = .value(.text(text)) }
        else if let array = try? container.decode([FixtureValue].self) { self = .value(.array(array.map(\.cbor))) }
        else {
            let map = try container.decode([String: FixtureValue].self)
            self = .value(.map(map.map { CBORMapEntry($0.key, $0.value.cbor) }))
        }
    }
    var cbor: CBORValue { switch self { case .value(let value): return value } }
}
final class FixtureClock {
    var started: UInt64?
    var now: UInt64 { started.map { (DispatchTime.now().uptimeNanoseconds - $0) / 1000 } ?? 0 }
}

func run() throws {
    let environment = ProcessInfo.processInfo.environment
    let configPath = environment["COGLET_FIXTURE_CONFIG"] ?? "spec/fixtures/surface/adapter-config.json"
    let config = try JSONDecoder().decode(FixtureValue.self, from: Data(contentsOf: URL(fileURLWithPath: configPath))).cbor
    guard case .text(let identity) = config["surface"], case .array(let entries) = config["grant"]?["entries"] else {
        throw SurfaceFailure.invalidFrame
    }
    let grants = try entries.map { entry -> EffectiveSurfaceGrant in
        guard case .text(let owner) = entry["coglet"], case .array(let operations) = entry["operations"],
              case .array(let resources) = entry["resources"], case .unsigned(let expires) = entry["expires"] else { throw SurfaceFailure.invalidFrame }
        func text(_ value: CBORValue) throws -> String {
            guard case .text(let string) = value else { throw SurfaceFailure.invalidFrame }; return string
        }
        return try .init(coglet: owner, operations: Set(operations.map(text)), resources: Set(resources.map(text)), expires: expires)
    }
    let base = environment["TMPDIR"] ?? FileManager.default.temporaryDirectory.path
    let directory = URL(fileURLWithPath: base).appendingPathComponent("coglet-swift-reference-" + UUID().uuidString)
    let store = try FileSurfaceLedger(directory: directory, newIdentity: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let name = environment["COGLET_CONFORMANCE_CASE"] ?? ""
    // Only documented provisioning labels select startup state, never responses.
    if case .unsigned(let epoch) = config["startup_epoch_by_case"]?[name], epoch > 1 {
        try store.save(CBORCodec.encodeProfile(.record([("identity", .text(identity)), ("epoch", .unsigned(epoch - 1)),
            ("sequence", .unsigned(0)), ("killed", .boolean(false)), ("records", .map([]))])))
    }
    let selected = Set(grants.flatMap { $0.operations })
    let backend = ReferenceBackend(config: config), clock = FixtureClock()
    let session = try SurfaceSession(identity: identity, boundCoglets: Set(grants.map(\.coglet)),
        operations: SurfaceOperation.builtins.filter { selected.contains($0.name) }, store: store, backend: backend,
        initialGrants: grants, clock: { clock.now }, correlation: {
            // Fixture supplies a stable hello/cap/grant correlation; event identity
            // is still distinguished by the durable monotonically increasing seq.
            if case .text(let id) = config["hello"]?["body"]?["correlation_id"] { return id }
            return UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        })
    defer { session.disconnect() }
    func flush() throws {
        for bytes in session.drain() { try FileHandle.standardOutput.write(contentsOf: Data(SurfaceWire.frame(bytes))) }
    }
    try session.start(); try flush()
    // Harness-only stderr handshake, after durable initialization and the first
    // wire frames. Loader/ledger startup uses the bounded connection budget,
    // not step zero's response budget. This is not a surface-protocol frame.
    try FileHandle.standardError.write(contentsOf: Data("COGLET_CONFORMANCE_READY\n".utf8))
    clock.started = DispatchTime.now().uptimeNanoseconds
    var decoder = SurfaceFrameDecoder()
    var killed = false
    while true {
        let now = clock.now
        if environment["COGLET_FIXTURE_KILL_MS"] == "100", !killed, now >= 100_000 {
            backend.stop(owner: nil); try session.kill(reason: "fixture native off-switch"); killed = true
        }
        backend.tick(now: now)
        try session.tick(); try flush()
        var descriptor = pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)
        let ready = poll(&descriptor, 1, 5)
        if ready < 0 {
            if errno == EINTR { continue }
            throw SurfaceFailure.system(errno)
        }
        if ready > 0 {
            var buffer = [UInt8](repeating: 0, count: 4096)
            let n = read(STDIN_FILENO, &buffer, buffer.count)
            if n == 0 { try decoder.finish(); return }
            if n < 0 { if errno == EINTR { continue }; throw SurfaceFailure.system(errno) }
            try decoder.consume(buffer.prefix(n)) { bytes in try session.receive(bytes) }
            try flush()
        }
    }
}
do { try run() }
catch {
    try? FileHandle.standardError.write(contentsOf: Data("Swift reference: \(error)\n".utf8))
    exit(1)
}
