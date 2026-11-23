import Foundation
import CogletTypes

public enum SurfaceFailure: Swift.Error, Equatable {
    case invalidFrame
    case closed
    case protocolViolation(String)
    case system(Int32)
    case unauthenticated
    case exhausted
}

/// The peer's role, not the caller's. Checked before dispatch, on both transports.
public enum SurfacePeer: Sendable {
    case runtime, surface
}

public enum SurfaceWire {
    public static let maximumFrame = 1_048_576
    public static let maximumQueuedMessages = 128

    public static func decode(_ bytes: [UInt8], from peer: SurfacePeer) throws -> CBORValue {
        guard !bytes.isEmpty, bytes.count <= maximumFrame else { throw SurfaceFailure.invalidFrame }
        let value = try CBORCodec.decode(rule: "surface::surface-message", bytes: bytes)
        guard case .text(let type) = value["type"] else { throw SurfaceFailure.invalidFrame }
        let allowed = peer == .runtime
            ? ["hello_reply", "read", "act", "health", "error"]
            : ["hello", "capabilities", "grants", "event", "read_reply", "accept", "health", "kill", "error"]
        guard allowed.contains(type) else { throw SurfaceFailure.protocolViolation("wrong message direction") }
        return value
    }

    public static func encode(_ value: CBORValue, from peer: SurfacePeer) throws -> [UInt8] {
        let bytes = try CBORCodec.encode(rule: "surface::surface-message", value: value)
        _ = try decode(bytes, from: peer)
        return bytes
    }

    public static func frame(_ bytes: [UInt8]) throws -> [UInt8] {
        guard !bytes.isEmpty, bytes.count <= maximumFrame else { throw SurfaceFailure.invalidFrame }
        let n = UInt32(bytes.count)
        return [UInt8(n >> 24), UInt8(truncatingIfNeeded: n >> 16),
                UInt8(truncatingIfNeeded: n >> 8), UInt8(truncatingIfNeeded: n)] + bytes
    }
}

/// Incremental framing. A peer may split every byte, or coalesce several frames.
/// The buffer never grows past one frame plus a four-byte header. Consumers must
/// drain each returned frame before reading further; this is not a hidden queue.
public struct SurfaceFrameDecoder {
    private var header: [UInt8] = []
    private var payload: [UInt8] = []
    private var length: Int?
    private var failed = false
    public init() {}

    public mutating func consume(_ bytes: ArraySlice<UInt8>, onFrame: ([UInt8]) throws -> Void) throws {
        guard !failed else { throw SurfaceFailure.closed }
        do {
            for byte in bytes {
                if let n = length {
                    payload.append(byte)
                    if payload.count == n {
                        let complete = payload
                        payload = []; header = []; length = nil
                        try onFrame(complete)
                    }
                } else {
                    header.append(byte)
                    if header.count == 4 {
                        let n = header.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
                        guard n > 0, n <= SurfaceWire.maximumFrame else { throw SurfaceFailure.invalidFrame }
                        length = Int(n)
                    }
                }
            }
        } catch {
            failed = true; header = []; payload = []; length = nil
            throw error
        }
    }

    public mutating func finish() throws {
        guard !failed, header.isEmpty, payload.isEmpty, length == nil else {
            failed = true
            throw SurfaceFailure.invalidFrame
        }
        failed = true
    }
}

extension CBORValue {
    public subscript(_ key: String) -> CBORValue? {
        guard case .map(let entries) = self else { return nil }
        return entries.first { $0.key.utf8.elementsEqual(key.utf8) }?.value
    }
    public static func record(_ entries: [(String, CBORValue)]) -> CBORValue {
        .map(entries.map { CBORMapEntry($0.0, $0.1) })
    }
    public static func message(_ type: String, _ body: CBORValue) -> CBORValue {
        .record([("type", .text(type)), ("body", body)])
    }
}
