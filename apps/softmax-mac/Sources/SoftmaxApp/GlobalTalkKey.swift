import AppKit
import CoreGraphics
import SoftmaxCore

/// Listen-only: never consumes another app's keystrokes. Tap lives on main run loop.
@MainActor final class GlobalTalkKey: PushToTalkKeySource {
    var onEvent: ((PushToTalkKeyEvent) -> Void)?
    var onRecorded: ((Int, UInt64) -> Void)?
    var recording = false { didSet { down = false; onEvent?(.cancelled) } }
    var keyCode: Int = 61
    var modifiers: UInt64 = 0
    private var down = false
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    static let modifierMask: UInt64 = CGEventFlags.maskCommand.rawValue | CGEventFlags.maskControl.rawValue |
        CGEventFlags.maskAlternate.rawValue | CGEventFlags.maskShift.rawValue | CGEventFlags.maskSecondaryFn.rawValue
    // Device-specific bits distinguish left/right modifiers, including both Options held.
    private static let sideBits: [Int: UInt64] = [54: 0x10, 55: 0x8, 56: 0x2, 60: 0x4,
        58: 0x20, 61: 0x40, 59: 0x1, 62: 0x2000, 63: 0x800000]

    func start() -> Bool {
        if tap != nil { return true }
        guard CGPreflightListenEventAccess() else { return false }
        let mask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue) |
            (1 << CGEventType.flagsChanged.rawValue)
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
            options: .listenOnly, eventsOfInterest: CGEventMask(mask), callback: { _, type, event, pointer in
                guard let pointer else { return Unmanaged.passUnretained(event) }
                MainActor.assumeIsolated {
                    Unmanaged<GlobalTalkKey>.fromOpaque(pointer).takeUnretainedValue().receive(type, event)
                }
                return Unmanaged.passUnretained(event)
            }, userInfo: pointer) else { return false }
        self.tap = tap
        source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        return true
    }
    func stop() {
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        if let tap { CFMachPortInvalidate(tap) }
        source = nil; tap = nil; down = false; onEvent?(.cancelled)
    }
    private func receive(_ type: CGEventType, _ event: CGEvent) {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            down = false; onEvent?(.cancelled)
            if CGPreflightListenEventAccess(), let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            else { onEvent?(.permissionDenied) }
            return
        }
        let code = Int(event.getIntegerValueField(.keyboardEventKeycode))
        let repeatKey = event.getIntegerValueField(.keyboardEventAutorepeat) != 0
        if recording {
            guard !repeatKey, type == .keyDown || (type == .flagsChanged &&
                event.flags.rawValue & (Self.sideBits[code] ?? 0) != 0) else { return }
            var flags = event.flags.rawValue & Self.modifierMask
            if Self.sideBits[code] != nil { flags = 0 }
            recording = false; onRecorded?(code, flags); return
        }
        let required = event.flags.rawValue & modifiers == modifiers
        let active: Bool
        if let side = Self.sideBits[keyCode] {
            active = event.flags.rawValue & side != 0 && required
        } else if code == keyCode {
            active = type == .keyDown && required
        } else {
            active = down && required
        }
        if active && !down && !repeatKey { down = true; onEvent?(.press) }
        else if !active && down { down = false; onEvent?(.release) }
    }
}
