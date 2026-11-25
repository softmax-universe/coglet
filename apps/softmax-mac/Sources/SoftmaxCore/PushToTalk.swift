import Foundation
import FaceMac

public enum PushToTalkKeyEvent: Sendable { case press, release, cancelled, permissionDenied }
@MainActor public protocol PushToTalkKeySource: AnyObject {
    var onEvent: ((PushToTalkKeyEvent) -> Void)? { get set }
    var eventTimestamp: TimeInterval? { get }
    func start() -> Bool
    func stop()
}
public extension PushToTalkKeySource { var eventTimestamp: TimeInterval? { nil } }
public enum TalkMode: String, Sendable { case hold, toggle }
public enum SpeechInput: Sendable, Equatable {
    case utterance(Utterance)
    case bargeIn(timestamp: TimeInterval)
}
public struct Utterance: Sendable, Equatable {
    public let operationID: UInt64
    public let frames: [PCMFrame]
    public let startedAt: TimeInterval
    public let endedAt: TimeInterval
}

/// Hardware-independent, main-actor serialized controller. All times are monotonic.
@MainActor public final class PushToTalk {
    public let speechInput: AsyncStream<SpeechInput>
    private let output: AsyncStream<SpeechInput>.Continuation
    private let keys: any PushToTalkKeySource
    private let audio: any AudioCapture
    private let now: () -> TimeInterval
    public var mode: TalkMode = .hold { didSet { cancel() } }
    public var permissions = PermissionSnapshot(microphone: .unknown, inputMonitoring: .unknown) {
        didSet { if !permissions.canUseGlobalPushToTalk { cancel() } }
    }
    public private(set) var state: AppState = .idle
    public private(set) var operationID: UInt64 = 0
    public private(set) var elapsed: TimeInterval = 0
    public var inputLevel: Float { state == .listening ? audio.inputLevel : 0 }
    public var onChange: (() -> Void)?
    public var routeToPermissions: (() -> Void)?
    public var permissionProvider: (() -> PermissionSnapshot)?
    public var stopPlayback: (() -> Void)?
    public var playStartSound: (() -> Void)?
    private var startedAt: TimeInterval?
    private var keyDown = false
    private var enabled = false
    private var timer: Task<Void, Never>?

    public init(keys: any PushToTalkKeySource, audio: any AudioCapture,
                now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
        self.keys = keys; self.audio = audio; self.now = now
        (speechInput, output) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(4))
    }
    public func start() {
        keys.onEvent = { [weak self] event in
            guard let self else { return }
            handle(event, at: keys.eventTimestamp)
        }
        enabled = keys.start()
        if !enabled { routeToPermissions?() }
    }
    public func shutdown() {
        enabled = false; cancel(); keys.stop(); keys.onEvent = nil
    }
    /// Runtime/Face sets speaking before playback starts. Leaving listening cancels capture.
    public func setState(_ state: AppState) {
        if state != .listening { cancel() }
        self.state = state; onChange?()
    }
    /// A delayed completion may only close the operation and phase that created it.
    @discardableResult
    public func complete(operation: UInt64, state expectedState: AppState) -> Bool {
        guard operation == operationID, state == expectedState,
              expectedState == .thinking || expectedState == .speaking else { return false }
        // A played-back completion is not cancellation. Invoking stopPlayback
        // here would turn the renderer's successful receipt into barge-in.
        operationID &+= 1; state = .idle; onChange?()
        return true
    }
    public func handle(_ event: PushToTalkKeyEvent, at timestamp: TimeInterval? = nil) {
        if case .press = event, let permissionProvider { permissions = permissionProvider() }
        switch event {
        case .permissionDenied:
            enabled = false; cancel(); routeToPermissions?()
        case .cancelled: cancel()
        case .release:
            keyDown = false
            if mode == .hold { finish(at: timestamp) }
        case .press:
            guard enabled, !keyDown else { return }
            keyDown = true
            if mode == .toggle && startedAt != nil { finish(at: timestamp); return }
            guard startedAt == nil else { return }
            guard permissions.canUseGlobalPushToTalk else { routeToPermissions?(); return }
            guard state != .onboarding else { routeToPermissions?(); return }
            let pressTime = timestamp ?? now()
            operationID &+= 1
            let wasSpeaking = state == .speaking
            // A new capture also fences replies pending synthesis or between
            // chunks. Only actual speaking emits a barge-in MESSAGE.
            stopPlayback?()
            if wasSpeaking {
                output.yield(.bargeIn(timestamp: now()))
            }
            do { try audio.start() }
            catch {
                _ = audio.stop(); state = .error; onChange?(); return
            }
            startedAt = pressTime; elapsed = 0; state = .listening
            playStartSound?(); onChange?()
            timer?.cancel()
            timer = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(50))
                    guard !Task.isCancelled, self != nil else { return }
                    self?.tick()
                }
            }
        }
    }
    public func tick() {
        if let permissionProvider { permissions = permissionProvider() }
        guard let startedAt else { return }
        if audio.captureFailed { cancel(); state = .error; onChange?(); return }
        elapsed = min(120, max(0, now() - startedAt))
        if elapsed >= 120 { finish() } else { onChange?() }
    }
    public func cancel() {
        operationID &+= 1
        let wasSpeaking = state == .speaking
        timer?.cancel(); timer = nil
        if startedAt != nil { _ = audio.stop() }
        if state == .listening || state == .thinking || wasSpeaking { state = .idle }
        startedAt = nil; elapsed = 0; keyDown = false; onChange?()
        if wasSpeaking { stopPlayback?() }
    }
    private func finish(at timestamp: TimeInterval? = nil) {
        guard let start = startedAt else { return }
        timer?.cancel(); timer = nil; startedAt = nil
        let end = min(timestamp ?? now(), start + 120)
        let captured = audio.stop()
        if audio.captureFailed { state = .error; onChange?(); return }
        elapsed = max(0, end - start)
        guard elapsed >= 0.250 else { state = .idle; onChange?(); return }
        // Trim any device callback that crosses the hard deadline.
        let frames = captured.compactMap { frame -> PCMFrame? in
            guard frame.sampleRate.isFinite, frame.sampleRate > 0, frame.timestamp < end else { return nil }
            let count = min(frame.samples.count, Int(max(0, (end - frame.timestamp) * frame.sampleRate)))
            return PCMFrame(samples: Array(frame.samples.prefix(count)), sampleRate: frame.sampleRate, timestamp: frame.timestamp)
        }
        guard frames.contains(where: { !$0.samples.isEmpty }) else {
            state = .idle; onChange?(); return
        }
        state = .thinking
        output.yield(.utterance(Utterance(operationID: operationID, frames: frames, startedAt: start, endedAt: end)))
        onChange?()
    }
}
