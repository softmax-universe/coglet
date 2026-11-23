import Foundation
import CogletTypes

/// Local normalization and conservative aligned-text projection. Native adapters
/// must capture owner/epoch/session at callback creation, not on callback arrival.
/// This object never infers heard text from generated text or duration ratios.
public final class SpeechObservations {
    public struct Span: Sendable {
        public let endMS: UInt64
        public let text: String
        public init(endMS: UInt64, text: String) { self.endMS = endMS; self.text = text }
    }
    private struct Utterance { var revision: UInt64 = 0; var closed = false }
    private struct Item {
        let owner: String
        let epoch: UInt64
        var spans: [Span] = []
        var played: UInt64 = 0
        var stopped = false
    }
    private var utterances: [String: Utterance] = [:]
    private var items: [String: Item] = [:]
    private let capacity: Int
    public init(capacity: Int = 1024) { self.capacity = capacity }

    public func start(utterance: String) throws {
        try CBORCodec.validate("common::correlation-id", .text(utterance))
        guard utterances[utterance] == nil else { throw NativeSurfaceError("conflict") }
        guard utterances.count < capacity else { throw NativeSurfaceError("resource_exhausted") }
        utterances[utterance] = Utterance()
    }
    /// false means discard silently with no private transcript forwarded/logged.
    public func transcript(utterance: String, revision: UInt64, text: String, final: Bool) throws -> Bool {
        guard var state = utterances[utterance] else { throw NativeSurfaceError("not_found") }
        guard text.utf8.count <= 16_384 else { throw NativeSurfaceError("invalid_argument") }
        guard !state.closed, revision > state.revision else { return false }
        state.revision = revision; state.closed = final; utterances[utterance] = state
        return true
    }
    public func register(item: String, owner: String, epoch: UInt64) throws {
        try CBORCodec.validate("common::correlation-id", .text(item))
        try CBORCodec.validate("common::coglet-id", .text(owner))
        guard epoch > 0, items[item] == nil else { throw NativeSurfaceError("conflict") }
        guard items.count < capacity else { throw NativeSurfaceError("resource_exhausted") }
        items[item] = Item(owner: owner, epoch: epoch)
    }
    /// Native alignment can ONLY append spans, never rewrite a previous prefix.
    /// No callback can add text to a stopped item or a later owner's item.
    public func append(item: String, owner: String, epoch: UInt64, spans: [Span]) throws {
        guard var state = items[item], state.owner == owner, state.epoch == epoch else { throw NativeSurfaceError("permission_denied") }
        guard !state.stopped else { throw NativeSurfaceError("conflict") }
        var end = state.spans.last?.endMS ?? 0
        var bytes = state.spans.reduce(0) { $0 + $1.text.utf8.count }
        guard state.spans.count + spans.count <= capacity else { throw NativeSurfaceError("resource_exhausted") }
        for span in spans {
            guard span.endMS > end else { throw NativeSurfaceError("invalid_argument") }
            bytes += span.text.utf8.count
            guard bytes <= 16_384 else { throw NativeSurfaceError("resource_exhausted") }
            end = span.endMS
        }
        state.spans.append(contentsOf: spans); items[item] = state
    }
    public func playback(item: String, owner: String, epoch: UInt64, playedMS: UInt64, stopped: Bool) throws {
        guard var state = items[item], state.owner == owner, state.epoch == epoch else { throw NativeSurfaceError("permission_denied") }
        guard playedMS >= state.played, !state.stopped || (playedMS == state.played && stopped) else {
            throw NativeSurfaceError("invalid_argument")
        }
        state.played = playedMS; state.stopped = stopped; items[item] = state
    }
    public func heard(item: String, owner: String, epoch: UInt64) throws -> CBORValue {
        guard let state = items[item], state.owner == owner, state.epoch == epoch else { throw NativeSurfaceError("permission_denied") }
        let prefix = state.spans.filter { $0.endMS <= state.played }.map(\.text).joined()
        return .record([("item", .text(item)), ("played_ms", .unsigned(state.played)), ("text", .text(prefix))])
    }
    public func revoke(owner: String) {
        items = items.filter { $0.value.owner != owner }
        // Utterances are private too; a capture adapter starts fresh IDs.
        utterances = [:]
    }
}
