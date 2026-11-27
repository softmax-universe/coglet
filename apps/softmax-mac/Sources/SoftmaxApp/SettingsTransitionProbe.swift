#if DEBUG
import AppKit
import Foundation
import SoftmaxCore

/// Bounded DEBUG native harness for Settings arrow-key transition order (design revision 2).
/// Never changes focus/selection, never calls handlers directly, never logs query text or responder addresses.
/// Events are real keyDown/keyUp NSEvents routed through NSApp.sendEvent / NSApp.postEvent only.
/// In-process events bypass XCTest/WindowServer: this cannot prove or disprove historical synthesis cause.
@MainActor
enum SettingsTransitionProbe {
    enum Scenario: String, Codable { case settled, queuedAfterSlash, queuedAfterQuery }
    enum Outcome: String, Codable { case pass, focusedFailure, transitionOnly, preconditionUnavailable, invalidRoute }
    enum Fixture: String, Codable { case empty, singleMatchSet, noMatches, other }
    enum Route: String, Codable { case notification, nativeEditing, unobserved }
    enum FailureKind: String, Codable { case missingHandler, wrongSelection, laterOverwrite }
    /// Fixed labels for every dispatched key; one outstanding keyDown per label.
    enum Label: String, Codable {
        case slash, queryO, firstDown, return1, down2, return2, up1, return3
        case selectAll, noMatchChar, noMatchDown, noMatchUp, noMatchReturn
        var spec: (code: UInt16, chars: String, flags: NSEvent.ModifierFlags) {
            func fn(_ k: Int) -> String { String(UnicodeScalar(UInt32(k))!) }
            let arrow: NSEvent.ModifierFlags = [.numericPad, .function]
            switch self {
            case .slash: return (44, "/", [])
            case .queryO: return (31, "o", [])
            case .firstDown, .down2, .noMatchDown: return (125, fn(NSDownArrowFunctionKey), arrow)
            case .up1, .noMatchUp: return (126, fn(NSUpArrowFunctionKey), arrow)
            case .return1, .return2, .return3, .noMatchReturn: return (36, "\r", [])
            case .selectAll: return (0, "a", .command)
            case .noMatchChar: return (6, "z", [])
            }
        }
        /// Handler phases the existing SwiftUI code emits for this key (nil: no handler expected, e.g. text).
        var handler: (SettingsArrowCapture.Phase, SettingsArrowCapture.Phase)? {
            switch spec.code {
            case 125: return (.downEnter, .downExit)
            case 126: return (.upEnter, .upExit)
            case 36: return (.submitEnter, .submitExit)
            default: return nil
            }
        }
    }
    enum Step {
        case waitKey, waitEditor, waitArrivals
        case waitQuery(Fixture)
        case armQueryPost
        case send(Label)
        case post([Label])
        case check(Label, SettingsTab)
    }
    struct Checkpoint: Codable {
        let sequence: Int
        let uptime: Double
        let kind: String          // fixed vocabulary only
        let label: Label?
        let editorOwned: Bool?
        let selected: String?
        let matchCount: Int?
    }
    struct Mismatch: Codable { let label: Label; let kind: FailureKind; let expected: String; let actual: String }
    struct Document: Encodable {
        let schema = 2
        let boundary = "transitionScenarioEnd"
        let scenario: Scenario
        let outcome: Outcome
        let complete: Bool
        let dropped: Int
        let checkpointOverflow: Bool
        let terminalReason: String
        let firstDownArrived: Bool
        let firstDownEditorOwned: Bool?
        let firstDownQueryReady: Bool
        let queryRoute: Route
        let unexpectedArrivals: Int
        let mismatches: [Mismatch]
        let checkpoints: [Checkpoint]
        let records: [SettingsArrowCapture.Record]
        let nativeCensus: SettingsNativeCensus.Document
    }

    // Bounds are harness-only: 10 ms ticks, 500 ticks and 5 s per checkpoint, 15 s per scenario.
    private static let tickInterval = 0.01, scenarioWall = 15.0
    private static let capacity = 128, settleTicks = 2
    private static let fixtureQuery = "o", noMatchQuery = "z"

    private static var scenario: Scenario?
    private static var outputURL: URL?
    private static weak var window: NSWindow?
    private static weak var model: ShellModel?
    private static var timer: Timer?
    private static var steps: [Step] = []
    private static var index = 0, stepTicks = 0, settle = 0
    private static var stepStart = 0.0, scenarioStart = 0.0
    private static var finished = false
    private static var checkpoints: [Checkpoint] = []
    private static var overflow = false
    private static var expected: [Label] = []
    private static var marks: [Label: Int] = [:]
    private static var unexpected = 0, invalidRoute = false, armed = false, notified = false
    private static var firstArrived = false, firstOwned: Bool?, firstReady = false
    private static var queryFixture = Fixture.empty, queryMatches = 0, route = Route.unobserved
    private static var mismatches: [Mismatch] = []

    // MARK: Hooks (called by AppDelegate/SettingsView, all under #if DEBUG)
    static func viewFocusChanged() {
        guard scenario != nil, !finished else { return }
        SettingsNativeCensus.capture(.viewFocusFirst, window: window, stepIndex: index, stepTicks: stepTicks)
    }

    static func attach(window: NSWindow, model: ShellModel) {
        let env = ProcessInfo.processInfo.environment
        guard scenario == nil, ProcessInfo.processInfo.arguments.contains("--ui-testing"),
              let name = env["SOFTMAX_SETTINGS_TRANSITION_SCENARIO"], let chosen = Scenario(rawValue: name),
              let path = env["SOFTMAX_SETTINGS_TRANSITION_OUTPUT"] else { return }
        scenario = chosen; outputURL = URL(fileURLWithPath: path)
        self.window = window; self.model = model
        scenarioStart = ProcessInfo.processInfo.systemUptime; stepStart = scenarioStart
        steps = plan(chosen)
        timer = Timer.scheduledTimer(withTimeInterval: tickInterval, repeats: true) { _ in
            MainActor.assumeIsolated { SettingsTransitionProbe.tick() }
        }
    }

    /// Local-monitor keyDown hook; runs before type-ahead/editing, so ownership here is the arrival state.
    static func arrival(_ event: NSEvent) {
        guard scenario != nil, !finished, let window, event.windowNumber == window.windowNumber else { return }
        guard let label = expected.first, label.spec.code == event.keyCode else {
            unexpected += 1; mark("unexpectedArrival", nil); return
        }
        expected.removeFirst()
        marks[label] = SettingsArrowCapture.records.count - 1
        let owned = ownership()
        mark("arrival", label, owned: owned)
        if label == .queryO { notified = false }
        if label == .firstDown {
            firstArrived = true; firstOwned = owned
            firstReady = queryFixture == .singleMatchSet && queryMatches == fixtureTabs(fixtureQuery).count
        }
    }

    /// onChange(of: query): records fixture enum only; queuedAfterQuery posts one Down to the queue tail.
    static func queryChanged(_ query: String, matchCount: Int) {
        guard scenario != nil, !finished else { return }
        queryFixture = query.isEmpty ? .empty : query == fixtureQuery ? .singleMatchSet : query == noMatchQuery ? .noMatches : .other
        queryMatches = matchCount
        mark("viewQuery", nil, matches: matchCount)
        if queryFixture == .singleMatchSet, route == .unobserved { route = notified ? .notification : .nativeEditing }
        if armed, queryFixture == .singleMatchSet { armed = false; post([.firstDown]) }  // post only, never sendEvent
    }

    /// onReceive(.softmaxFilterInput) after query mutation.
    static func notificationReceived() {
        guard scenario != nil, !finished else { return }
        notified = true; mark("notification", nil)
    }

    // MARK: Plan
    private static func plan(_ scenario: Scenario) -> [Step] {
        var result: [Step] = [.waitKey, .send(.slash)]
        switch scenario {
        case .settled:
            result += [.waitEditor, .send(.queryO), .waitQuery(.singleMatchSet), .waitEditor, .send(.firstDown), .waitArrivals]
        case .queuedAfterSlash:
            result += [.waitArrivals, .waitQuery(.singleMatchSet)]
        case .queuedAfterQuery:
            result += [.waitEditor, .armQueryPost, .send(.queryO), .waitQuery(.singleMatchSet), .waitArrivals]
        }
        result += [.check(.firstDown, .hotkey),
                   .send(.return1), .check(.return1, .hotkey), .send(.down2), .check(.down2, .audio),
                   .send(.return2), .check(.return2, .audio), .send(.up1), .check(.up1, .hotkey),
                   .send(.return3), .check(.return3, .hotkey),
                   .send(.selectAll), .send(.noMatchChar), .waitQuery(.noMatches),
                   .send(.noMatchDown), .check(.noMatchDown, .hotkey), .send(.noMatchUp), .check(.noMatchUp, .hotkey),
                   .send(.noMatchReturn), .check(.noMatchReturn, .hotkey)]
        return result
    }

    // MARK: State machine (one step per 10 ms main-run-loop tick; no blocking waits)
    private static func tick() {
        guard !finished else { timer?.invalidate(); return }
        let now = ProcessInfo.processInfo.systemUptime
        if now - scenarioStart >= scenarioWall { return finish("scenarioDeadline") }
        guard index < steps.count else { return finish("complete") }
        if SettingsArrowCapture.outputURL == nil { return finish("collectorDisabled") }
        stepTicks += 1
        if SettingsProbeDeadline.expired(now: now, start: stepStart, ticks: stepTicks) {
            return finish("checkpointTimeout")
        }
        let done = run(steps[index])
        if finished { return }
        let completedAt = ProcessInfo.processInfo.systemUptime
        if completedAt - scenarioStart >= scenarioWall { return finish("scenarioDeadline") }
        if SettingsProbeDeadline.expired(now: completedAt, start: stepStart, ticks: stepTicks) {
            return finish("checkpointTimeout")
        }
        if done { index += 1; stepTicks = 0; settle = 0; stepStart = completedAt }
    }

    private static func run(_ step: Step) -> Bool {
        switch step {
        case .waitKey: return window?.isKeyWindow == true && model != nil
        case .waitEditor:
            SettingsNativeCensus.capture(.waitEditorFirst, window: window, stepIndex: index, stepTicks: stepTicks)
            return ownership() == true
        case .waitArrivals: return expected.isEmpty
        case .waitQuery(let fixture): return queryFixture == fixture && (fixture != .noMatches || queryMatches == 0)
        case .armQueryPost: armed = true; return true
        case .send(let label):
            send(label)
            if expected.contains(label) { invalidRoute = true; finish("monitorNotReached") }  // sendEvent must reach monitor synchronously
            if label == .slash, scenario == .queuedAfterSlash, !finished { post([.queryO, .firstDown]) }
            return true
        case .post(let labels): post(labels); return true
        case .check(let label, let tab):
            guard expected.isEmpty else { return false }
            settle += 1
            guard settle >= settleTicks else { return false }
            verify(label, tab)
            return true
        }
    }

    // MARK: Event dispatch
    private static func events(_ label: Label) -> [NSEvent] {
        guard let window else { return [] }
        let spec = label.spec
        return [NSEvent.EventType.keyDown, .keyUp].compactMap {
            NSEvent.keyEvent(with: $0, location: .zero, modifierFlags: spec.flags,
                             timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                             context: nil, characters: spec.chars, charactersIgnoringModifiers: spec.chars,
                             isARepeat: false, keyCode: spec.code)
        }
    }
    private static func send(_ label: Label) {
        let list = events(label)
        guard list.count == 2 else { invalidRoute = true; return finish("eventCreation") }
        marks[label] = SettingsArrowCapture.records.count
        expected.append(label); mark("sendBegin", label)
        list.forEach { NSApp.sendEvent($0) }
        mark("sendEnd", label)
    }
    private static func post(_ labels: [Label]) {
        for label in labels {
            let list = events(label)
            guard list.count == 2 else { invalidRoute = true; return finish("eventCreation") }
            marks[label] = SettingsArrowCapture.records.count
            expected.append(label); mark("postBegin", label)
            list.forEach { NSApp.postEvent($0, atStart: false) }
            mark("postEnd", label)
        }
    }

    // MARK: Read-only observations
    /// true only when the unique native placeholder field has an editor identical to firstResponder.
    /// Hosted census proved SwiftUI's AX label is not the underlying NSTextField's label.
    /// nil: field missing/ambiguous or traversal incomplete. Never creates an editor or touches focus.
    private static func ownership() -> Bool? {
        guard let window, let root = window.contentView else { return nil }
        // Same traversal/storage/depth bounds as SettingsNativeCensus; truncation refuses qualification.
        var stack: [(NSView, Int)] = [(root, 0)]
        var fields: [NSTextField] = []
        var visited = 0
        while let (view, depth) = stack.popLast() {
            guard visited < 256 else { return nil }
            visited += 1
            if let field = view as? NSTextField, field.placeholderString == "Filter settings  /" {
                fields.append(field)
                if fields.count > 1 { return nil }
            }
            let children = view.subviews
            if depth >= 32 {
                if !children.isEmpty { return nil }
            } else {
                let remaining = max(0, 256 - visited - stack.count)
                guard children.count <= remaining else { return nil }
                stack.append(contentsOf: children.map { ($0, depth + 1) })
            }
        }
        guard fields.count == 1 else { return nil }
        let field = fields[0]
        guard field.isEditable, field.isEnabled, !field.isHiddenOrHasHiddenAncestor,
              let editor = field.currentEditor(), let responder = window.firstResponder else { return false }
        return editor === responder
    }
    private static func fixtureTabs(_ query: String) -> [SettingsTab] {
        SettingsTab.allCases.filter { ($0.rawValue + " " + $0.keywords).localizedCaseInsensitiveContains(query) }
    }
    private static func verify(_ label: Label, _ tab: SettingsTab) {
        let actual = model?.selectedTab
        mark("check", label, selected: actual?.rawValue)
        var kind = FailureKind.missingHandler
        var handlerCorrect = false
        if let phases = label.handler, let start = marks[label] {
            let seen = SettingsArrowCapture.records.dropFirst(start)
            if let enter = seen.first(where: { $0.phase == phases.0 }) {
                let exit = seen.first(where: { $0.phase == phases.1 && $0.sequence > enter.sequence })
                handlerCorrect = exit?.selected == tab.rawValue
                kind = handlerCorrect ? .laterOverwrite : .wrongSelection
            }
        }
        guard actual != tab || !handlerCorrect else { return }
        mismatches.append(.init(label: label, kind: kind, expected: tab.rawValue, actual: actual?.rawValue ?? "nil"))
    }
    private static func mark(_ kind: String, _ label: Label?, owned: Bool? = nil, matches: Int? = nil, selected: String? = nil) {
        guard checkpoints.count < capacity else { overflow = true; return }
        checkpoints.append(.init(sequence: checkpoints.count, uptime: ProcessInfo.processInfo.systemUptime, kind: kind,
                                 label: label, editorOwned: owned, selected: selected ?? model?.selectedTab.rawValue, matchCount: matches))
    }

    // MARK: Terminal
    private static func finish(_ reason: String) {
        guard !finished, let scenario else { return }
        finished = true; timer?.invalidate(); timer = nil
        SettingsNativeCensus.capture(.terminal, window: window, stepIndex: index, stepTicks: stepTicks)
        let dropped = SettingsArrowCapture.dropped
        let evidenceComplete = !overflow && dropped == 0 && SettingsArrowCapture.outputURL != nil
        let timedOut = !["complete", "monitorNotReached", "eventCreation"].contains(reason)
        let general = fixtureTabs(fixtureQuery)
        let fixtureOK = [SettingsTab.general, .hotkey, .audio].allSatisfy(general.contains)
        let preconditionsMet = firstArrived && firstOwned == true && firstReady
        let preconditionsUnavailable = !firstArrived || firstOwned == nil || !fixtureOK
        let outcome: Outcome
        if unexpected > 0 || invalidRoute { outcome = .invalidRoute }
        else if timedOut || !evidenceComplete || preconditionsUnavailable { outcome = .preconditionUnavailable }
        else if preconditionsMet { outcome = mismatches.isEmpty ? .pass : .focusedFailure }
        else { outcome = scenario == .settled ? .preconditionUnavailable : .transitionOnly }
        let document = Document(scenario: scenario, outcome: outcome, complete: evidenceComplete, dropped: dropped,
                                checkpointOverflow: overflow, terminalReason: reason, firstDownArrived: firstArrived,
                                firstDownEditorOwned: firstOwned, firstDownQueryReady: firstReady, queryRoute: route,
                                unexpectedArrivals: unexpected, mismatches: mismatches, checkpoints: checkpoints,
                                records: SettingsArrowCapture.records, nativeCensus: SettingsNativeCensus.document)
        // One atomic write; a missing file is an explicit driver failure.
        if let outputURL, let data = try? JSONEncoder().encode(document) { try? data.write(to: outputURL, options: .atomic) }
    }
}
#endif
