import Foundation
import CogletSurface

/// Application-supplied production symbols. The unchecked transfer is limited
/// to immutable C function pointers; only the worker calls any of them.
public struct FaceRuntimeABI: @unchecked Sendable {
    public typealias JSONCall = @convention(c) (UnsafePointer<UInt8>?, Int) -> Int32
    public typealias StatusCall = @convention(c) () -> Int32
    let surface: SurfaceRuntimeABI
    let configure: JSONCall
    let start: JSONCall
    let stop: StatusCall
    let status: StatusCall
    public init(surface: SurfaceRuntimeABI, configure: @escaping JSONCall,
                start: @escaping JSONCall, stop: @escaping StatusCall, status: @escaping StatusCall) {
        self.surface = surface; self.configure = configure; self.start = start
        self.stop = stop; self.status = status
    }
}

public enum FaceRuntimeFailure: Swift.Error, Sendable { case busy, closed, runtime, oversized }

/// Bounded metadata only: never retains binding/configuration/frame bytes or
/// arbitrary error descriptions. A nil status means the SDK did not expose it.
public struct FaceRuntimeDiagnostic: Swift.Error, Sendable, Equatable {
    public enum Phase: String, Sendable { case configure, start, open, send, receive, stop, status }
    public enum Cause: String, Sendable {
        case abi, invalidSizeQuery, invalidExactRead, invalidFrame, closed
        case protocolViolation, system, unauthenticated, exhausted, unknown
    }
    public let phase: Phase
    public let cause: Cause
    public let status: Int32?

    init(phase: Phase, cause: Cause = .abi, status: Int32? = nil) {
        self.phase = phase; self.cause = cause; self.status = status
    }
    init(phase: Phase, error: any Swift.Error) {
        self.phase = phase
        var cause: Cause = .unknown
        var status: Int32?
        if let failure = error as? SurfaceFailure {
            switch failure {
            case .invalidFrame: cause = .invalidFrame
            case .closed: cause = .closed
            case .system(let value): cause = .system; status = value
            case .unauthenticated: cause = .unauthenticated
            case .exhausted: cause = .exhausted
            case .protocolViolation(let message):
                cause = .protocolViolation
                // Only parse the SDK's fixed ABI metadata, never copy its text.
                let prefix = phase == .open ? "ABI open failed: " : "ABI send failed: "
                if (phase == .open || phase == .send), message.hasPrefix(prefix),
                   let value = Int32(message.dropFirst(prefix.count)) {
                    cause = .abi; status = value
                } else if message == "invalid ABI size query" { cause = .invalidSizeQuery }
                else if message == "invalid ABI exact read" { cause = .invalidExactRead }
            }
        }
        self.cause = cause; self.status = status
    }
}

/// One process-wide ABI supervisor. A failed stop retains custody: a successor
/// must not configure a runtime whose termination could not be established.
private final class FaceRuntimeLease: @unchecked Sendable {
    static let shared = FaceRuntimeLease()
    private let lock = NSLock()
    private var owner: UUID?
    func acquire(_ id: UUID) throws {
        try lock.withLock { guard owner == nil else { throw FaceRuntimeFailure.busy }; owner = id }
    }
    func release(_ id: UUID) { lock.withLock { if owner == id { owner = nil } } }
}

/// Lock covers bounded copied mailboxes only, NEVER an ABI call or disk access.
/// The serial queue owns every pointer, the timer, and runtime teardown. This
/// unchecked conformance does not make EmbeddedSurfaceTransport thread-safe.
private final class FaceRuntimeWorker: @unchecked Sendable {
    let id = UUID()
    let abi: FaceRuntimeABI
    private let queue = DispatchQueue(label: "org.coglet.face.runtime", qos: .userInitiated)
    private let lock = NSLock()
    private struct OutgoingFrame {
        let bytes: [UInt8]
        let isEvent: Bool
    }
    private var outgoing: [OutgoingFrame] = []
    // Lock-protected: once selected, the head stays fixed across ABI execution
    // and would-block retries until success (or mailbox closure).
    private var outgoingHeadPinned = false
    private var incoming: [[UInt8]] = []
    private var outgoingBytes = 0
    private var incomingBytes = 0
    private var closed = false
    private var failure: FaceRuntimeDiagnostic?
    private var stoppedOK = false
    private var stopFailure: FaceRuntimeDiagnostic?
    // Queue-confined fields below.
    private var transport: EmbeddedSurfaceTransport?
    private var timer: DispatchSourceTimer?
    private var needsStop = false
    private var leased = false
    private var cleaned = false
    private static let byteLimit = 8 * 1024 * 1024
    init(abi: FaceRuntimeABI) { self.abi = abi }
    func start(binding: [UInt8], configuration: [UInt8]) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Swift.Error>) in
            queue.async { [self] in
                do {
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    try FaceRuntimeLease.shared.acquire(id); leased = true
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    // Even configure can catch a native panic and fence global
                    // state. Any lifecycle attempt needs stop/status attestation.
                    needsStop = true
                    let configured = binding.withUnsafeBufferPointer { abi.configure($0.baseAddress, $0.count) }
                    guard configured == 0 else { throw FaceRuntimeDiagnostic(phase: .configure, status: configured) }
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    let started = configuration.withUnsafeBufferPointer { abi.start($0.baseAddress, $0.count) }
                    guard started == 0 else { throw FaceRuntimeDiagnostic(phase: .start, status: started) }
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    do { transport = try EmbeddedSurfaceTransport(abi: abi.surface) }
                    catch { throw FaceRuntimeDiagnostic(phase: .open, error: error) }
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    let source = DispatchSource.makeTimerSource(queue: queue)
                    source.schedule(deadline: .now(), repeating: .milliseconds(5))
                    source.setEventHandler { [weak self] in self?.pump() }
                    timer = source; source.resume()
                    continuation.resume()
                } catch {
                    lock.withLock { failure = error as? FaceRuntimeDiagnostic; closed = true }
                    cleanup(); continuation.resume(throwing: error)
                }
            }
        }
    }
    func send(_ bytes: [UInt8]) throws -> Bool {
        // Encoding is checked on the caller before copying; the worker validates
        // once more at its actual ABI boundary through the SDK transport.
        guard bytes.count <= SurfaceWire.maximumFrame else { throw FaceRuntimeFailure.oversized }
        let message = try SurfaceWire.decode(bytes, from: .surface)
        let isEvent = message["type"] == .text("event")
        let isReadReply = message["type"] == .text("read_reply")
        return try lock.withLock {
            guard !closed else { throw failure.map { $0 as any Swift.Error } ?? FaceRuntimeFailure.closed }
            guard outgoing.count < 128, outgoingBytes + bytes.count <= Self.byteLimit else { return false }
            var index = outgoing.count
            if isReadReply {
                // Only cross the contiguous trailing event run. Every other
                // frame (including another reply) and the pinned head is a fence.
                let floor = outgoingHeadPinned ? 1 : 0
                while index > floor && outgoing[index - 1].isEvent { index -= 1 }
            }
            outgoing.insert(.init(bytes: bytes, isEvent: isEvent), at: index)
            outgoingBytes += bytes.count; return true
        }
    }
    func receive() throws -> [UInt8]? {
        try lock.withLock {
            guard !closed else { throw failure.map { $0 as any Swift.Error } ?? FaceRuntimeFailure.closed }
            guard !incoming.isEmpty else { return nil }
            let result = incoming.removeFirst(); incomingBytes -= result.count; return result
        }
    }
    func flush() throws -> Bool {
        try lock.withLock {
            guard !closed else { throw failure.map { $0 as any Swift.Error } ?? FaceRuntimeFailure.closed }
            return outgoing.isEmpty
        }
    }
    func close() {
        let first = lock.withLock {
            if closed { return false }
            closed = true; incoming = []; outgoing = []; outgoingHeadPinned = false; incomingBytes = 0; outgoingBytes = 0
            return true
        }
        if first { queue.async { [self] in cleanup() } }
    }
    func shutdown() async throws {
        close()
        let success = await withCheckedContinuation { continuation in
            queue.async { [self] in cleanup(); continuation.resume(returning: lock.withLock { stoppedOK }) }
        }
        guard success else { throw lock.withLock { stopFailure.map { $0 as any Swift.Error } ?? FaceRuntimeFailure.runtime } }
    }
    private func pump() {
        guard !lock.withLock({ closed }), let transport else { cleanup(); return }
        var phase = FaceRuntimeDiagnostic.Phase.send
        do {
            for _ in 0..<16 {
                guard !lock.withLock({ closed }) else { cleanup(); return }
                if let first = lock.withLock({ () -> OutgoingFrame? in
                    guard !closed, let first = outgoing.first else { return nil }
                    outgoingHeadPinned = true
                    return first
                }) {
                    phase = .send
                    if try transport.send(first.bytes) {
                        lock.withLock {
                            // Insertion cannot move the pinned head. close may
                            // have erased it while ABI send ran; never pop then.
                            if !closed {
                                outgoing.removeFirst(); outgoingBytes -= first.bytes.count
                                outgoingHeadPinned = false
                            }
                        }
                    }
                }
                guard !lock.withLock({ closed }) else { cleanup(); return }
                let room = lock.withLock { incoming.count < 128 && incomingBytes <= Self.byteLimit - SurfaceWire.maximumFrame }
                // Full inbound mailbox applies backpressure without consuming or
                // discarding runtime frames. MainActor keeps its independent stop.
                guard room else { return }
                phase = .receive
                let next = try transport.receive()
                if let next {
                    lock.withLock { if !closed { incoming.append(next); incomingBytes += next.count } }
                }
                if next == nil { break }
            }
        } catch {
            let diagnostic = FaceRuntimeDiagnostic(phase: phase, error: error)
            lock.withLock { failure = diagnostic; closed = true; incoming = []; outgoing = []; outgoingHeadPinned = false; incomingBytes = 0; outgoingBytes = 0 }
            cleanup()
        }
    }
    private func cleanup() {
        guard !cleaned else { return }; cleaned = true
        timer?.cancel(); timer = nil
        transport?.close(); transport = nil
        var diagnostic: FaceRuntimeDiagnostic?
        if needsStop {
            let stopped = abi.stop()
            if stopped != 0 { diagnostic = .init(phase: .stop, status: stopped) }
            else {
                let status = abi.status()
                if status != 1 { diagnostic = .init(phase: .status, status: status) }
            }
        }
        let stopped = diagnostic == nil
        needsStop = false
        lock.withLock { stoppedOK = stopped; stopFailure = diagnostic }
        if leased && stopped { FaceRuntimeLease.shared.release(id); leased = false }
    }
}

/// Embeddable MainActor mailbox. Actual configure/start/open/send/recv/close/stop
/// run solely on a bounded serial worker, never on MainActor. `close()` fences
/// private bytes immediately and never waits for a scheduled guest. The app may
/// await `shutdown()` AFTER the native stop reflex to attest runtime teardown.
@MainActor public final class FaceEmbeddedRuntimeLink: FaceControlLink {
    private let worker: FaceRuntimeWorker
    private init(worker: FaceRuntimeWorker) { self.worker = worker }
    public static func start(abi: FaceRuntimeABI, binding: [UInt8], configuration: [UInt8]) async throws -> FaceEmbeddedRuntimeLink {
        let worker = FaceRuntimeWorker(abi: abi)
        try await withTaskCancellationHandler {
            try await worker.start(binding: binding, configuration: configuration)
            try Task.checkCancellation()
        } onCancel: { worker.close() }
        return FaceEmbeddedRuntimeLink(worker: worker)
    }
    deinit { worker.close() }
    public func send(_ bytes: [UInt8]) throws -> Bool { try worker.send(bytes) }
    public func receive() throws -> [UInt8]? { try worker.receive() }
    public func flush() throws -> Bool { try worker.flush() }
    public func close() { worker.close() }
    public func shutdown() async throws { try await worker.shutdown() }
}
