import Foundation
import FaceMac

/// Product ADAPTER renderer. One synthesis at a time, bounded PCM, and native
/// queue backpressure. A successful callback means Face's played-back callback.
@MainActor public final class RuntimeSpeechPlayback {
    private struct Item {
        let key = UUID()
        let text: String
        let stream: String
        let final: Bool
        let complete: (String?) -> Void
    }
    private struct Stream { var next: UInt64 = 0; var closed = false }
    private let session: FacePlaybackSession
    private let synthesis: any LocalSpeechSynthesis
    private var streams: [String: Stream] = [:]
    private var activeStream: String?
    private var queue: [Item] = []
    private var current: Item?
    private var token: UUID?
    private var frames: [PCMFrame] = []
    private var offset = 0
    private var cancelSynthesis: (@MainActor () -> Void)?
    private var feeding: Task<Void, Never>?
    private var idleSince: TimeInterval?
    public var willBegin: (() -> Void)?
    public init(session: FacePlaybackSession, synthesis: any LocalSpeechSynthesis) {
        self.session = session; self.synthesis = synthesis
        session.onResult = { [weak self] cancelled in
            guard let self, let item = current else { return }
            current = nil; token = nil; frames = []; offset = 0
            feeding?.cancel(); feeding = nil; cancelSynthesis = nil
            if cancelled {
                item.complete("cancelled"); stop()
            } else {
                if item.final { activeStream = nil }
                else { idleSince = ProcessInfo.processInfo.systemUptime }
                item.complete(nil); startNext()
            }
        }
    }
    public func enqueue(text: String, stream: String, chunk: UInt64, final: Bool,
                        completion: @escaping (String?) -> Void) {
        if let idleSince, ProcessInfo.processInfo.systemUptime - idleSince >= 60 { stop() }
        guard !text.isEmpty, text.utf8.count <= 4096 else { completion("invalid_argument"); return }
        guard queue.count + (current == nil ? 0 : 1) < 16 else { completion("resource_exhausted"); return }
        guard streams[stream] != nil || streams.count < 4096 else { completion("resource_exhausted"); return }
        var state = streams[stream] ?? Stream()
        guard !state.closed, state.next == chunk, chunk < 64,
              activeStream == nil || activeStream == stream else { completion("conflict"); return }
        state.next += 1; state.closed = final; streams[stream] = state
        activeStream = stream; idleSince = nil
        queue.append(Item(text: text, stream: stream, final: final, complete: completion))
        startNext()
    }
    public func stop() {
        let old = current; let waiting = queue
        current = nil; queue = []; token = nil; frames = []; offset = 0
        feeding?.cancel(); feeding = nil
        let cancel = cancelSynthesis; cancelSynthesis = nil
        if let activeStream { streams[activeStream]?.closed = true }
        activeStream = nil; idleSince = nil
        cancel?(); session.stop()
        old?.complete("cancelled")
        for item in waiting { item.complete("cancelled") }
    }
    private func startNext() {
        guard current == nil, !queue.isEmpty else { return }
        let item = queue.removeFirst(); current = item
        willBegin?()
        token = session.begin()
        let cancel = synthesis.synthesize(item.text) { [weak self] result in
            guard let self, current?.key == item.key else { return }
            switch result {
            case .failure: fail("unavailable")
            case .success(let pcm):
                guard !pcm.isEmpty, pcm.count <= 8192,
                      pcm.reduce(0, { $0 + $1.samples.count }) <= 1_920_000 else { fail("resource_exhausted"); return }
                frames = pcm; offset = 0
                feed()
                guard current?.key == item.key, offset < frames.count else { return }
                feeding = Task { [weak self] in
                    while !Task.isCancelled {
                        try? await Task.sleep(for: .milliseconds(20))
                        guard !Task.isCancelled, let self, self.current?.key == item.key else { return }
                        self.session.speech.tick(); self.feed()
                        if self.offset >= self.frames.count { return }
                    }
                }
            }
        }
        // A synchronous failure/completion cannot leave a stale cancellation.
        if current?.key == item.key { cancelSynthesis = cancel }
    }
    private func feed() {
        guard current != nil, let token else { return }
        while offset < frames.count {
            do { try session.speech.enqueue(frames[offset], token: token); offset += 1 }
            catch FacePlaybackError.capacity { return } // retry unchanged PCM after native drain
            catch { fail("unavailable"); return }
        }
        do { try session.speech.finish(token: token) }
        catch { fail("unavailable") }
    }
    private func fail(_ code: String) {
        guard let item = current else { return }
        current = nil
        item.complete(code)
        stop() // a failed chunk closes its stream and cancels successors
    }
}
