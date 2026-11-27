import Foundation
import CogletSurface
import CogletTypes

/// Durable per-item heard projection (speech.history). Same observable rules as
/// `SpeechObservations` (monotone spans, conservative prefix, stopped items
/// freeze) but every register/append/playback/revoke is saved through a
/// `SurfaceLedgerStore` BEFORE it is visible or returned. Records are retained
/// indefinitely (stronger than the 24h action-ledger minimum); a full history
/// refuses new items instead of evicting. Heard text is only ever aligned span
/// text supplied by the adapter, never generated text or a duration estimate.
///
/// Pass a store distinct from the SDK action ledger (e.g. a second
/// `FileSurfaceLedger` directory); the snapshot format is not the SDK format.
/// `nil` store = in-memory, for unadvertised use only.
///
/// Failure model: a corrupt/unreadable store throws from `init` (no fallback to
/// empty). A failed save (or failed `revokeAll`) latches this instance
/// unavailable: in-memory data is dropped and every later call throws
/// `unavailable` until a fresh instance is built. The caller must not enable new
/// bindings after a failed `revokeAll`.
@MainActor
public final class FaceHeardHistory {
    public static let itemCapacity = 1024
    public static let snapshotVersion: UInt64 = 1
    static let maxTextBytes = 16_384

    private struct Item {
        let owner: String
        let epoch: UInt64
        var spans: [SpeechObservations.Span] = []
        var played: UInt64 = 0
        var stopped = false
    }

    private let store: (any SurfaceLedgerStore)?
    private var items: [String: Item] = [:]
    private var latched = false

    init() { store = nil }

    public init(store: (any SurfaceLedgerStore)?) throws {
        self.store = store
        guard let store else { return }
        let bytes = try store.load()
        let root = try CBORCodec.decodeProfile(bytes)
        if root == .map([]) { return } // pristine ledger
        items = try Self.restore(root)
    }

    public var isAvailable: Bool { !latched }

    public func register(item: String, owner: String, epoch: UInt64) throws {
        try requireAvailable()
        try Self.validateIDs(item: item, owner: owner)
        guard epoch > 0, items[item] == nil else { throw NativeSurfaceError("conflict") }
        guard items.count < Self.itemCapacity else { throw NativeSurfaceError("resource_exhausted") }
        var next = items
        next[item] = Item(owner: owner, epoch: epoch)
        try commit(next)
    }

    /// Append-only aligned spans. Stopped items never accept text.
    public func append(item: String, owner: String, epoch: UInt64, spans: [SpeechObservations.Span]) throws {
        try requireAvailable()
        var state = try bound(item, owner, epoch)
        guard !state.stopped else { throw NativeSurfaceError("conflict") }
        var end = state.spans.last?.endMS ?? 0
        var bytes = state.spans.reduce(0) { $0 + $1.text.utf8.count }
        guard state.spans.count + spans.count <= Self.itemCapacity else { throw NativeSurfaceError("resource_exhausted") }
        for span in spans {
            guard span.endMS > end else { throw NativeSurfaceError("invalid_argument") }
            bytes += span.text.utf8.count
            guard bytes <= Self.maxTextBytes else { throw NativeSurfaceError("resource_exhausted") }
            end = span.endMS
        }
        state.spans.append(contentsOf: spans)
        var next = items
        next[item] = state
        try commit(next)
    }

    public func playback(item: String, owner: String, epoch: UInt64, playedMS: UInt64, stopped: Bool) throws {
        try requireAvailable()
        var state = try bound(item, owner, epoch)
        guard playedMS >= state.played, !state.stopped || (playedMS == state.played && stopped) else {
            throw NativeSurfaceError("invalid_argument")
        }
        if state.played == playedMS, state.stopped == stopped { return }
        state.played = playedMS; state.stopped = stopped
        var next = items
        next[item] = state
        try commit(next)
    }

    public func heard(item: String, owner: String, epoch: UInt64) throws -> CBORValue {
        try requireAvailable()
        let state = try bound(item, owner, epoch)
        let prefix = state.spans.filter { $0.endMS <= state.played }.map(\.text).joined()
        return .record([("item", .text(item)), ("played_ms", .unsigned(state.played)), ("text", .text(prefix))])
    }

    /// Durably drops every item. Failure latches unavailable (and drops memory);
    /// the previous durable snapshot may then still hold private text, so the
    /// caller must refuse new bindings.
    public func revokeAll() throws {
        try requireAvailable()
        try commit([:])
    }

    // MARK: - internals

    private func requireAvailable() throws {
        guard !latched else { throw NativeSurfaceError("unavailable") }
    }

    private func bound(_ item: String, _ owner: String, _ epoch: UInt64) throws -> Item {
        guard let state = items[item] else { throw NativeSurfaceError("not_found") }
        guard state.owner == owner, state.epoch == epoch else { throw NativeSurfaceError("permission_denied") }
        return state
    }

    private static func validateIDs(item: String, owner: String) throws {
        do {
            try CBORCodec.validate("common::correlation-id", .text(item))
            try CBORCodec.validate("common::coglet-id", .text(owner))
        } catch { throw NativeSurfaceError("invalid_argument") }
    }

    private func commit(_ next: [String: Item]) throws {
        if let store {
            do {
                let bytes = try Self.encode(next)
                guard bytes.count <= 268_435_456 else { throw SurfaceFailure.exhausted }
                try store.save(bytes)
            } catch {
                latched = true
                items = [:]
                throw error
            }
        }
        items = next
    }

    private static func encode(_ items: [String: Item]) throws -> [UInt8] {
        let rows = items.sorted { $0.key < $1.key }.map { key, item in
            CBORMapEntry(key, .record([
                ("owner", .text(item.owner)), ("epoch", .unsigned(item.epoch)),
                ("spans", .array(item.spans.map { .record([("end_ms", .unsigned($0.endMS)), ("text", .text($0.text))]) })),
                ("played_ms", .unsigned(item.played)), ("stopped", .boolean(item.stopped))]))
        }
        return try CBORCodec.encodeProfile(.record([
            ("version", .unsigned(snapshotVersion)), ("items", .map(rows))]))
    }

    private static func corrupt() -> SurfaceFailure { .protocolViolation("corrupt heard history") }

    private static func restore(_ root: CBORValue) throws -> [String: Item] {
        guard case .map(let top) = root, top.count == 2,
              root["version"] == .unsigned(snapshotVersion),
              case .map(let rows)? = root["items"], rows.count <= itemCapacity else { throw corrupt() }
        var result: [String: Item] = [:]
        for row in rows {
            let id = row.key
            guard case .map(let fields) = row.value, fields.count == 5,
                  case .text(let owner)? = row.value["owner"],
                  case .unsigned(let epoch)? = row.value["epoch"], epoch > 0,
                  case .array(let rawSpans)? = row.value["spans"], rawSpans.count <= itemCapacity,
                  case .unsigned(let played)? = row.value["played_ms"],
                  case .boolean(let stopped)? = row.value["stopped"],
                  result[id] == nil else { throw corrupt() }
            do { try validateIDs(item: id, owner: owner) } catch { throw corrupt() }
            var item = Item(owner: owner, epoch: epoch)
            var end: UInt64 = 0
            var bytes = 0
            for raw in rawSpans {
                guard case .map(let spanFields) = raw, spanFields.count == 2,
                      case .unsigned(let endMS)? = raw["end_ms"], endMS > end,
                      case .text(let text)? = raw["text"] else { throw corrupt() }
                bytes += text.utf8.count
                guard bytes <= maxTextBytes else { throw corrupt() }
                end = endMS
                item.spans.append(.init(endMS: endMS, text: text))
            }
            item.played = played; item.stopped = stopped
            result[id] = item
        }
        return result
    }
}
