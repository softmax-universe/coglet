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
    private var outgoing: [[UInt8]] = []
    private var incoming: [[UInt8]] = []
    private var outgoingBytes = 0
    private var incomingBytes = 0
    private var closed = false
    private var failed = false
    private var stoppedOK = false
    // Queue-confined fields below.
    private var transport: EmbeddedSurfaceTransport?
    private var timer: DispatchSourceTimer?
    private var started = false
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
                    guard binding.withUnsafeBufferPointer({ abi.configure($0.baseAddress, $0.count) }) == 0 else { throw FaceRuntimeFailure.runtime }
                    guard configuration.withUnsafeBufferPointer({ abi.start($0.baseAddress, $0.count) }) == 0 else { throw FaceRuntimeFailure.runtime }
                    started = true
                    transport = try EmbeddedSurfaceTransport(abi: abi.surface)
                    guard !lock.withLock({ closed }) else { throw FaceRuntimeFailure.closed }
                    let source = DispatchSource.makeTimerSource(queue: queue)
                    source.schedule(deadline: .now(), repeating: .milliseconds(5))
                    source.setEventHandler { [weak self] in self?.pump() }
                    timer = source; source.resume()
                    continuation.resume()
                } catch {
                    lock.withLock { failed = true; closed = true }
                    cleanup(); continuation.resume(throwing: FaceRuntimeFailure.runtime)
                }
            }
        }
    }
    func send(_ bytes: [UInt8]) throws -> Bool {
        // Encoding is checked on the caller before copying; the worker validates
        // once more at its actual ABI boundary through the SDK transport.
        guard bytes.count <= SurfaceWire.maximumFrame else { throw FaceRuntimeFailure.oversized }
        _ = try SurfaceWire.decode(bytes, from: .surface)
        return try lock.withLock {
            guard !closed else { throw failed ? FaceRuntimeFailure.runtime : .closed }
            guard outgoing.count < 128, outgoingBytes + bytes.count <= Self.byteLimit else { return false }
            outgoing.append(bytes); outgoingBytes += bytes.count; return true
        }
    }
    func receive() throws -> [UInt8]? {
        try lock.withLock {
            guard !closed else { throw failed ? FaceRuntimeFailure.runtime : .closed }
            guard !incoming.isEmpty else { return nil }
            let result = incoming.removeFirst(); incomingBytes -= result.count; return result
        }
    }
    func flush() throws -> Bool {
        try lock.withLock {
            guard !closed else { throw failed ? FaceRuntimeFailure.runtime : .closed }
            return outgoing.isEmpty
        }
    }
    func close() {
        let first = lock.withLock {
            if closed { return false }
            closed = true; incoming = []; outgoing = []; incomingBytes = 0; outgoingBytes = 0
            return true
        }
        if first { queue.async { [self] in cleanup() } }
    }
    func shutdown() async throws {
        close()
        let success = await withCheckedContinuation { continuation in
            queue.async { [self] in cleanup(); continuation.resume(returning: lock.withLock { stoppedOK }) }
        }
        guard success else { throw FaceRuntimeFailure.runtime }
    }
    private func pump() {
        guard !lock.withLock({ closed }), let transport else { cleanup(); return }
        do {
            for _ in 0..<16 {
                guard !lock.withLock({ closed }) else { cleanup(); return }
                if let first = lock.withLock({ outgoing.first }) {
                    if try transport.send(first) {
                        lock.withLock {
                            // close may have erased the mailbox while ABI send ran.
                            if !closed { outgoing.removeFirst(); outgoingBytes -= first.count }
                        }
                    }
                }
                guard !lock.withLock({ closed }) else { cleanup(); return }
                let room = lock.withLock { incoming.count < 128 && incomingBytes <= Self.byteLimit - SurfaceWire.maximumFrame }
                // Full inbound mailbox applies backpressure without consuming or
                // discarding runtime frames. MainActor keeps its independent stop.
                guard room else { return }
                let next = try transport.receive()
                if let next {
                    lock.withLock { if !closed { incoming.append(next); incomingBytes += next.count } }
                }
                if next == nil { break }
            }
        } catch {
            lock.withLock { failed = true; closed = true; incoming = []; outgoing = []; incomingBytes = 0; outgoingBytes = 0 }
            cleanup()
        }
    }
    private func cleanup() {
        guard !cleaned else { return }; cleaned = true
        timer?.cancel(); timer = nil
        transport?.close(); transport = nil
        let stopped = !started || (abi.stop() == 0 && abi.status() == 1)
        started = false
        lock.withLock { stoppedOK = stopped }
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
