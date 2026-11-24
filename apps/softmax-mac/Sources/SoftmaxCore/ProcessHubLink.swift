import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// Bundled real-iroh transport. Secrets only cross anonymous pipes.
@MainActor
public final class ProcessHubLink: HubLink {
    private let executable: URL
    private var process: Process?
    private var input: FileHandle?
    private var output: FileHandle?
    private var reader: Task<Void, Never>?
    private var callback: ((HubLinkEvent) -> Void)?
    private var generation: UInt64 = 0
    public init(executable: URL) { self.executable = executable }
    public func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws {
        stop()
        guard key.count == 32 else { throw HubError.identity }
        let child = Process(), stdin = Pipe(), stdout = Pipe()
        #if canImport(Darwin)
        // A helper can close stdin before a command. Suppress SIGPIPE only on
        // this descriptor, preserving the application's global signal policy.
        guard fcntl(stdin.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1) != -1 else {
            throw HubError.unavailable
        }
        #endif
        child.executableURL = executable
        child.arguments = []
        child.standardInput = stdin; child.standardOutput = stdout
        child.standardError = FileHandle.nullDevice
        let current = generation
        callback = event
        child.terminationHandler = { [weak self] _ in
            Task { @MainActor in self?.deliver(.failed, generation: current) }
        }
        try child.run()
        process = child; input = stdin.fileHandleForWriting; output = stdout.fileHandleForReading
        let handle = stdout.fileHandleForReading
        reader = Task.detached { [weak self] in
            do {
                var pending = Data()
                var buffer = [UInt8](repeating: 0, count: 4096)
                while !Task.isCancelled {
                    // read(2) returns currently available bytes from a pipe; a
                    // Foundation fill-to-count read can stall a short ready frame.
                    let count = buffer.withUnsafeMutableBytes { bytes in
                        read(handle.fileDescriptor, bytes.baseAddress!, bytes.count)
                    }
                    if count < 0 {
                        if errno == EINTR { continue }
                        throw HubError.unavailable
                    }
                    if count == 0 { break }
                    pending.append(contentsOf: buffer.prefix(count))
                    guard pending.count <= 16384 else { throw HubError.unavailable }
                    while let newline = pending.firstIndex(of: 10) {
                        let line = pending[..<newline]
                        let object = try JSONSerialization.jsonObject(with: Data(line)) as? [String: String]
                        let event: HubLinkEvent
                        switch object?["state"] {
                        case "ready":
                            guard let device = object?["device"], HubPairing.isHex(device) else { throw HubError.unavailable }
                            event = .ready(device: device)
                        case "connected": event = .connected
                        case "offline": event = .offline
                        default: throw HubError.unavailable
                        }
                        pending.removeSubrange(...newline)
                        await self?.deliver(event, generation: current)
                    }
                }
            } catch { /* Input and parse errors must not disclose secrets. */ }
            await self?.deliver(.failed, generation: current)
        }
        do { try send(["key": key.map { String(format: "%02x", $0) }.joined()]) }
        catch { stop(); throw HubError.unavailable }
    }
    private func deliver(_ event: HubLinkEvent, generation current: UInt64) {
        guard current == generation, process != nil else { return }
        callback?(event)
        if case .failed = event { stop() }
    }
    private func send(_ object: [String: Any]) throws {
        guard let input else { throw HubError.unavailable }
        var data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        data.append(10)
        guard data.count <= 16384 else { throw HubError.pairing }
        do { try input.write(contentsOf: data) } catch { throw HubError.unavailable }
    }
    public func connect(_ pairing: HubPairing) throws {
        var command: [String: Any] = ["hub": pairing.hub, "addresses": pairing.addresses]
        if let relay = pairing.relay { command["relay"] = relay }
        if let invite = pairing.invite { command["invite"] = invite }
        try send(command)
    }
    /// Close only the current QUIC connection; retain endpoint identity for recovery.
    public func disconnect() throws { try send([:]) }
    public func stop() {
        generation &+= 1
        callback = nil; reader?.cancel(); reader = nil
        try? input?.close(); input = nil
        if let process {
            process.terminationHandler = nil
            if process.isRunning { process.terminate() }
        }
        process = nil
        try? output?.close(); output = nil
    }
}
