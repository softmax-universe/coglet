import Foundation
import FaceMac
import SoftmaxCore

@MainActor final class TestClock { var time = 10.0 }
@MainActor final class FakeFacePlayback: FacePlaybackDevice {
    var renderedFrames: Int64 = 0
    var playing = false
    var completions: [@MainActor () -> Void] = []
    var onStop: (() -> Void)?
    func schedule(_ frame: PCMFrame, completion: @escaping @MainActor () -> Void) throws {
        playing = true; completions.append(completion)
    }
    func stop() { playing = false; renderedFrames = 0; onStop?() }
}
@MainActor final class FaceTestKeys: PushToTalkKeySource {
    var onEvent: ((PushToTalkKeyEvent) -> Void)?
    func start() -> Bool { true }
    func stop() {}
    func send(_ event: PushToTalkKeyEvent) { onEvent?(event) }
}
@MainActor final class FaceTestAudio: AudioCapture {
    let frames = AsyncStream<PCMFrame> { _ in }
    var inputLevel: Float { 0 }
    func start() throws {}
    func stop() -> [PCMFrame] { [] }
}
