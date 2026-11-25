// ADR-0034 source-bound experiment; not historical failure attribution.
import Foundation
import Dispatch
import Darwin
import CogletTypes
import CogletSurface

typealias Configure = @convention(c) (UnsafePointer<UInt8>?, Int) -> Int32
typealias Stop = @convention(c) () -> Int32
typealias Hold = @convention(c) (OpaquePointer?) -> OpaquePointer?
typealias Release = @convention(c) (OpaquePointer?) -> Void
typealias Entries = @convention(c) (OpaquePointer?) -> Int
let library = dlopen(CommandLine.arguments[1], RTLD_NOW | RTLD_LOCAL)!
func symbol<T>(_ name: String, _: T.Type) -> T {
    guard let pointer = dlsym(library, name) else { fatalError("missing symbol \(name)") }
    return unsafeBitCast(pointer, to: T.self)
}
let realOpen = symbol("coglet_surface_open", SurfaceRuntimeABI.Open.self)
let realClose = symbol("coglet_surface_close", SurfaceRuntimeABI.Close.self)
let realRecv = symbol("coglet_surface_recv", SurfaceRuntimeABI.Recv.self)
var rawLink: OpaquePointer?
// SDK may close on its receive worker. Defer only allocation release until join;
// its own transport still becomes closed immediately. No production SDK edit.
var closeRequests = 0
var injectReceiveError = false
func deferredClose(_ link: OpaquePointer?) {
    require(link == rawLink, "close identity")
    closeRequests += 1
}
func diagnosticRecv(_ link: OpaquePointer?, _ out: UnsafeMutablePointer<UInt8>?, _ cap: Int,
                    _ n: UnsafeMutablePointer<Int>?) -> Int32 {
    if injectReceiveError { n?.pointee = 0; return -2 }
    return realRecv(link, out, cap, n)
}
var openBefore: UInt64 = 0
var openAfter: UInt64 = 0
func trackingOpen(_ version: UInt32, _ out: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 {
    openBefore = now()
    let status = realOpen(version, out); rawLink = out?.pointee
    openAfter = now()
    return status
}
let hold = symbol("coglet_surface_test_hold", Hold.self)
let release = symbol("coglet_surface_test_release", Release.self)
let entries = symbol("coglet_surface_test_entries", Entries.self)
let freeHold = symbol("coglet_surface_test_free_hold", Release.self)
let timings = symbol("coglet_surface_test_timings", SurfaceRuntimeABI.Recv.self)
func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds / 1000 }
func require(_ condition: Bool, _ message: String) {
    if !condition { fatalError(message) }
}
final class Backend: SurfaceNativeBackend {
    var stops = 0
    var effects = 0
    var lastStop: UInt64 = 0
    func preflight(action: CBORValue, now: UInt64) throws {}
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        effects += 1; return .succeeded(.map([]))
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        throw NativeSurfaceError("unavailable")
    }
    func stop(owner: String?) { stops += 1; lastStop = now() }
}
// Only this box crosses queues; SDK state remains on the main control thread.
final class Reply: @unchecked Sendable {
    let lock = NSLock()
    var result: Result<[UInt8]?, Swift.Error>?
    func put(_ value: Result<[UInt8]?, Swift.Error>) { lock.lock(); result = value; lock.unlock() }
    func get() -> Result<[UInt8]?, Swift.Error>? { lock.lock(); defer { lock.unlock() }; return result }
}
let root = URL(fileURLWithPath: CommandLine.arguments[2])
let obstruct = CommandLine.arguments[3] == "obstruct"
let owner = "coglet:" + String(repeating: "a", count: 64)
let identity = "surface:" + String(repeating: "b", count: 64)
let configure = symbol("coglet_runtime_configure", Configure.self)
let start = symbol("coglet_runtime_start", Configure.self)
let stop = symbol("coglet_runtime_stop", Stop.self)
let binding = Array(try Data(contentsOf: root.appendingPathComponent("binding.json")))
require(binding.withUnsafeBufferPointer { configure($0.baseAddress, $0.count) } == 0, "configure")
let startup = Array(try Data(contentsOf: root.appendingPathComponent("startup.json")))
require(startup.withUnsafeBufferPointer { start($0.baseAddress, $0.count) } == 0, "start")
let backend = Backend()
let session = try SurfaceSession(identity: identity, boundCoglets: [owner],
    operations: SurfaceOperation.builtins.filter { $0.name == "expression.set_expression" },
    store: FileSurfaceLedger(directory: root.appendingPathComponent("sdk-ledger"), newIdentity: true),
    backend: backend, initialGrants: [.init(coglet: owner, operations: ["expression.set_expression"],
        resources: ["display.face"], expires: now() + 60_000_000)], clock: now)
let transport = try EmbeddedSurfaceTransport(abi: .init(open: trackingOpen,
    send: symbol("coglet_surface_send", SurfaceRuntimeABI.Send.self),
    recv: diagnosticRecv,
    close: deferredClose))
func flush() throws {
    for bytes in session.drain() { require(try transport.send(bytes), "send would block") }
}
try session.start(); try flush()
guard let hello = try transport.receive() else { fatalError("no hello reply") }
// Calibration precedes the actual SDK receipt sample (never subtract epochs).
let baseline = now()
try session.receive(hello); try flush() // grants run the real no-effect WAMR guest
let stopsBefore = backend.stops
let control = obstruct ? hold(rawLink) : nil
if obstruct { require(control != nil, "hold installation") }
Thread.sleep(forTimeInterval: 1.05)
let box = Reply()
let joined = DispatchSemaphore(value: 0)
let abiEntry = now()
injectReceiveError = CommandLine.arguments[3] == "receive-error"
DispatchQueue.global().async {
    do { box.put(.success(try transport.receive())) } catch { box.put(.failure(error)) }
    joined.signal()
}
var receipt: UInt64?
var timeout: UInt64?
var loopError: Swift.Error?
// Eight-second pump watchdog is production-test-hook bounded. This outer loop
// never touches the exclusive ABI link until the worker signals completion.
do {
while now() - baseline < 3_100_000 {
    if receipt == nil, let result = box.get() {
        guard let bytes = try result.get() else { fatalError("expected naturally due health") }
        require(try SurfaceWire.decode(bytes, from: .runtime)["type"] == .text("health"), "not health")
        try session.receive(bytes); receipt = now()
    }
    do { try session.tick() }
    catch SurfaceFailure.protocolViolation("health timeout") {
        timeout = now(); break
    }
    Thread.sleep(forTimeInterval: 0.005)
}
} catch { loopError = error }
let loopExit = now()
let stopAt = backend.lastStop
let pumpEntries = control.map { entries($0) } ?? 0
let stopObserved = backend.stops > stopsBefore
if let control { release(control) }
require(joined.wait(timeout: .now() + 9) == .success, "ABI worker watchdog")
let automaticCloses = closeRequests
// Snapshot, close and runtime stop ONLY after join, even on the RED path.
var n = 0
require(timings(rawLink, nil, 0, &n) == 2 && n < 32768, "timing query")
var capture = [UInt8](repeating: 0, count: n)
let cap = n
require(capture.withUnsafeMutableBufferPointer { timings(rawLink, $0.baseAddress, cap, &n) } == 0, "timing copy")
print("ADR0034 calibration per_link_zero_sdk_bracket_us=[\(openBefore),\(openAfter)]")
print("ADR0034 SOURCE-BOUND mode=\(obstruct ? "obstruct" : "control") sdk_baseline_us=\(baseline) abi_entry_sdk_us=\(abiEntry) receipt_sdk_us=\(String(describing: receipt)) timeout_sdk_us=\(String(describing: timeout)) paired_capture_sdk_us=\(now()) entries=\(pumpEntries) stop=\(stopObserved) effects=\(backend.effects)")
print(String(decoding: capture, as: UTF8.self))
if let control { freeHold(control) }
transport.close()
require(closeRequests == 1, "SDK close callback exactly once")
realClose(rawLink); rawLink = nil
try session.close(); require(stop() == 0, "runtime stop after exclusive close")
if injectReceiveError {
    guard case SurfaceFailure.protocolViolation("invalid ABI size query")? = loopError else {
        fatalError("receive-error control missed SDK error path")
    }
    require(automaticCloses == 1 && receipt == nil && backend.effects == 0,
        "receive-error control did not exercise SDK automatic close")
    print("SDK_RECEIVE_ERROR_CLOSE_EXECUTED")
    exit(0)
}
if let loopError { throw loopError }
require(backend.effects == 0, "pre-heard effect escaped")
if let timeout {
    require(stopAt <= timeout && timeout - stopAt < 50_000 && loopExit - timeout < 50_000,
        "SDK local stop callback not prompt (environment threshold, not physical I/O proof)")
    require(timeout - baseline >= 3_000_000 && stopObserved && receipt == nil && pumpEntries > 0,
        "timeout without source-bound obstruction/native stop")
    print("SDK_TIMEOUT_NATIVE_STOP_EXECUTED")
    exit(2) // expected RED for old pump-first ordering, never a passing result
}
require(receipt != nil && (!obstruct || pumpEntries == 0), "ready health not delivered before timeout")
print("SDK_READY_HEALTH_EXECUTED")
