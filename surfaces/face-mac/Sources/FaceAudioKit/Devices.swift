import AVFoundation
import AudioToolbox
import CoreAudio

public struct AudioDevice: Equatable, Sendable {
    public let uid: String
    public let name: String
    public let hasInput: Bool
    public let hasOutput: Bool
}

public enum AudioDevices {
    private static func address(_ selector: AudioObjectPropertySelector,
                                scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal)
        -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope,
                                   mElement: kAudioObjectPropertyElementMain)
    }
    private static func ids() throws -> [AudioDeviceID] {
        var a = address(kAudioHardwarePropertyDevices)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size))
        guard size > 0 else { return [] }
        var devices = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        try devices.withUnsafeMutableBytes {
            try check(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, $0.baseAddress!))
        }
        return devices
    }
    private static func text(_ id: AudioDeviceID, _ selector: AudioObjectPropertySelector) throws -> String {
        var a = address(selector)
        var result: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        try check(AudioObjectGetPropertyData(id, &a, 0, nil, &size, &result))
        guard let result else { throw FaceAudioError.deviceUnavailable("missing device property") }
        return result.takeRetainedValue() as String
    }
    private static func supports(_ id: AudioDeviceID, _ scope: AudioObjectPropertyScope) -> Bool {
        var a = address(kAudioDevicePropertyStreams, scope: scope)
        var size: UInt32 = 0
        return AudioObjectGetPropertyDataSize(id, &a, 0, nil, &size) == noErr && size > 0
    }
    public static func list() throws -> [AudioDevice] {
        try ids().map { id in
            AudioDevice(uid: try text(id, kAudioDevicePropertyDeviceUID),
                        name: try text(id, kAudioObjectPropertyName),
                        hasInput: supports(id, kAudioDevicePropertyScopeInput),
                        hasOutput: supports(id, kAudioDevicePropertyScopeOutput))
        }
    }
    // A nil UID keeps the engine's system default. Called before VoiceProcessingIO
    // is enabled, because enabling it can replace the underlying AudioUnit.
    static func select(_ uid: String?, input: Bool, engine: AVAudioEngine) throws {
        guard let uid else { return }
        let scope = input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput
        guard var id = try ids().first(where: {
            try text($0, kAudioDevicePropertyDeviceUID) == uid && supports($0, scope)
        }) else { throw FaceAudioError.deviceUnavailable(uid) }
        let unit = input ? engine.inputNode.audioUnit : engine.outputNode.audioUnit
        guard let unit else { throw FaceAudioError.deviceUnavailable(uid) }
        try check(AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                                      kAudioUnitScope_Global, 0, &id,
                                      UInt32(MemoryLayout<AudioDeviceID>.size)))
    }
    private static func check(_ status: OSStatus) throws {
        if status != noErr { throw FaceAudioError.coreAudio(status) }
    }
}

public enum MicrophonePermission {
    public static var status: AVAuthorizationStatus { AVCaptureDevice.authorizationStatus(for: .audio) }
    public static func request() async -> Bool { await AVCaptureDevice.requestAccess(for: .audio) }
}
