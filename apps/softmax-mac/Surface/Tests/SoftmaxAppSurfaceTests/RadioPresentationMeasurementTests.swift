#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxAppSurface

/// Disposable measurement-only fixture. A pass is NOT visual qualification.
/// No product action, real account, system AX access, or persistent defaults.
@MainActor
final class RadioPresentationMeasurementTests: XCTestCase {
    // Measurement interventions, not a proposed product implementation.
    private enum Treatment: String, CaseIterable {
        case pickerOriginal = "picker-original", pickerMarked = "picker-marked"
        case ownedOriginal = "owned-original", ownedControl = "owned-control"
        case ownedStructural = "owned-structural"
        case ownedBaselineGap5 = "owned-baseline-gap5"
        case ownedCenteredDefaultGap = "owned-centered-default-gap", ownedCentered = "owned-centered"
        case nestedOriginal = "nested-original"
        // Staged feasibility probe: only these three run (3 x 6 sizes x 2 themes = 36 cases).
        static let probeCases: [Treatment] = [.pickerOriginal, .pickerMarked, .nestedOriginal]
        var nested: Bool { self == .nestedOriginal }
        var structural: Bool {
            self == .ownedStructural || self == .ownedBaselineGap5 || self == .ownedCenteredDefaultGap || self == .ownedCentered
        }
        var centered: Bool { self == .ownedCenteredDefaultGap || self == .ownedCentered }
        var gap5: Bool { self == .ownedBaselineGap5 || self == .ownedCentered }
        var owned: Bool { self == .ownedOriginal || self == .ownedControl || structural }
    }
    private static let appearanceChoices: [CogletRadioGroup.Choice] = [
        .init(value: "System", label: "System", id: "settings.general.appearance.system"),
        .init(value: "Light", label: "Light", id: "settings.general.appearance.light"),
        .init(value: "Dark", label: "Dark", id: "settings.general.appearance.dark")]
    private static let sizeChoices: [CogletRadioGroup.Choice] = [
        .init(value: "standard", label: "Standard", id: "settings.general.text_size.standard"),
        .init(value: "large", label: "Large", id: "settings.general.text_size.large"),
        .init(value: "largest", label: "Largest", id: "settings.general.text_size.largest")]
    @MainActor
    private struct Marker: NSViewRepresentable {
        let name: String
        func makeNSView(context: Context) -> NSView {
            let v = NSView(); v.identifier = NSUserInterfaceItemIdentifier(name); return v
        }
        func updateNSView(_ v: NSView, context: Context) {}
    }
    @MainActor
    private final class NestedContainer: NSView {
        /// Measurement-only container owning an NSHostingView of the ORIGINAL Picker.
        /// No stack view, offsets or frame formulas; native sizing forwarded, layout fills bounds.
        let hostingView: NSHostingView<AnyView>
        init(root: AnyView, name: String) {
            hostingView = NSHostingView(rootView: root)
            super.init(frame: .zero)
            identifier = NSUserInterfaceItemIdentifier("nestedhost." + name)
            addSubview(hostingView)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) not used") }
        override var intrinsicContentSize: NSSize { hostingView.intrinsicContentSize }
        override func layout() {
            super.layout()
            hostingView.frame = bounds
        }
    }
    @MainActor
    private struct NestedHost: NSViewRepresentable {
        let name: String
        let root: AnyView
        func makeNSView(context: Context) -> NestedContainer { NestedContainer(root: root, name: name) }
        func updateNSView(_ view: NestedContainer, context: Context) {} // No mutation, no rootView replacement.
        // This FIRST static feasibility probe uses the original Picker's ideal size,
        // not a claimed arbitrary-proposal bridge. Any resulting mismatch is negative.
        func sizeThatFits(_ proposal: ProposedViewSize, nsView: NestedContainer, context: Context) -> CGSize? {
            let ideal = nsView.hostingView.intrinsicContentSize
            guard ideal.width >= 0, ideal.height >= 0,
                  ideal.width.isFinite, ideal.height.isFinite else { return nil }
            return ideal
        }
    }
    @MainActor
    private struct Fixture: View {
        let treatment: Treatment, dark: Bool, pointSize: CGFloat
        var owned: Bool { treatment.owned }
        private var appearanceChoices: [CogletRadioGroup.Choice] { RadioPresentationMeasurementTests.appearanceChoices }
        private var sizeChoices: [CogletRadioGroup.Choice] { RadioPresentationMeasurementTests.sizeChoices }
        let registry: ControlRegistry
        // Immutable bindings: no SwiftUI updates may reconfigure the installed graph.
        private let appearance = Binding.constant("System")
        private let scale = Binding.constant(1.0)
        // Original Picker bodies, shared verbatim by the direct and nested-host paths.
        @ViewBuilder private var originalAppearancePicker: some View {
            Picker("Appearance", selection: appearance) {
                ForEach(appearanceChoices, id: \.value) { option($0.label, id: $0.id) }
            }.pickerStyle(.radioGroup).labelsHidden()
        }
        @ViewBuilder private var originalSizePicker: some View {
            Picker("Text size", selection: scale) {
                option("Standard", id: sizeChoices[0].id).tag(1.0)
                option("Large", id: sizeChoices[1].id).tag(1.3)
                option("Largest", id: sizeChoices[2].id).tag(1.6)
            }.pickerStyle(.radioGroup).labelsHidden()
        }
        // Nested NSHostingView root: explicit font/foreground/tint/colorScheme, nothing inherited.
        private func nestedRoot<V: View>(_ picker: V) -> AnyView {
            AnyView(picker.font(.system(size: pointSize)).foregroundColor(ink).foregroundStyle(ink).tint(accent)
                .environment(\.colorScheme, dark ? .dark : .light))
        }
        @ViewBuilder private func option(_ label: String, id: String) -> some View {
            if treatment == .pickerMarked {
                Text(label).background(Marker(name: "label." + id))
            } else { Text(label) }
        }
        // Exact generated tokens, not approximate appearance guesses.
        var ink: Color { dark ? Color(.sRGB, red: 0.925490196, green: 0.960784314, blue: 0.937254902, opacity: 1) : Color(.sRGB, red: 0.090196078, green: 0.137254902, blue: 0.113725490, opacity: 1) }
        var surface: Color { dark ? Color(.sRGB, red: 0.109803922, green: 0.149019608, blue: 0.125490196, opacity: 1) : .white }
        var background: Color { dark ? Color(.sRGB, red: 0.066666667, green: 0.090196078, blue: 0.078431373, opacity: 1) : Color(.sRGB, red: 0.956862745, green: 0.968627451, blue: 0.960784314, opacity: 1) }
        var border: Color { dark ? Color(.sRGB, red: 0.239215686, green: 0.305882353, blue: 0.258823529, opacity: 1) : Color(.sRGB, red: 0.815686275, green: 0.858823529, blue: 0.831372549, opacity: 1) }
        var accent: Color { dark ? Color(.sRGB, red: 0.529411765, green: 0.937254902, blue: 0.737254902, opacity: 1) : Color(.sRGB, red: 0.090196078, green: 0.407843137, blue: 0.258823529, opacity: 1) }
        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Appearance").font(.system(size: pointSize)).accessibilityHidden(true)
                    if owned {
                        CogletRadioGroup("Appearance", id: "settings.general.appearance", registry: registry,
                            window: "settings", route: "general", choices: appearanceChoices,
                            selection: appearance, fontSize: pointSize, failure: { XCTFail("\($0)") })
                            .background(Marker(name: "appearance.rows"))
                    } else if treatment.nested {
                        NestedHost(name: "appearance", root: nestedRoot(originalAppearancePicker))
                            .background(Marker(name: "appearance.rows"))
                    } else {
                        originalAppearancePicker.background(Marker(name: "appearance.rows"))
                    }
                }.background(Marker(name: "appearance.group"))
                VStack(alignment: .leading, spacing: 4) {
                    Text("Text size").font(.system(size: pointSize)).accessibilityHidden(true)
                    if owned {
                        CogletRadioGroup("Text size", id: "settings.general.text_size", registry: registry,
                            window: "settings", route: "general", choices: sizeChoices,
                            selection: .constant("standard"),
                            fontSize: pointSize, failure: { XCTFail("\($0)") })
                            .background(Marker(name: "text_size.rows"))
                    } else if treatment.nested {
                        NestedHost(name: "text_size", root: nestedRoot(originalSizePicker))
                            .background(Marker(name: "text_size.rows"))
                    } else {
                        originalSizePicker.background(Marker(name: "text_size.rows"))
                    }
                }.background(Marker(name: "text_size.group"))
                Text("System Standard").background(Marker(name: "reference.text"))
            }
                // DesignGroup: spacing[2] == 12, padding spacing[3] == 16.
                .foregroundColor(ink).frame(maxWidth: .infinity, alignment: .leading).padding(16)
                .background(surface, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(border, lineWidth: 1))
                .padding(16).frame(width: 540, height: 440, alignment: .topLeading)
                .font(.system(size: pointSize)).foregroundStyle(ink)
                .tint(accent).background(background)
                .environment(\.colorScheme, dark ? .dark : .light)
        }
    }
    private func rect(_ r: NSRect) -> [Double] { [r.origin.x, r.origin.y, r.width, r.height].map(Double.init) }
    private func size(_ s: NSSize) -> [Double] { [Double(s.width), Double(s.height)] }
    private func color(_ c: NSColor?) -> Any {
        guard let c = c?.usingColorSpace(.deviceRGB) else { return "notExposed" }
        return [Double(c.redComponent), Double(c.greenComponent), Double(c.blueComponent), Double(c.alphaComponent)]
    }
    private func font(_ f: NSFont?) -> Any {
        guard let f else { return "notExposed" }
        return ["name": f.fontName, "size": Double(f.pointSize), "ascender": Double(f.ascender),
                "descender": Double(f.descender), "capHeight": Double(f.capHeight), "xHeight": Double(f.xHeight),
                "descriptor": f.fontDescriptor.description] as [String: Any]
    }
    private func metadata(_ v: NSView, host: NSView, path: String) -> [String: Any] {
        let ins = v.alignmentRectInsets
        var d: [String: Any] = ["path": path, "class": NSStringFromClass(type(of: v)),
            "id": v.identifier?.rawValue ?? "", "frame_parent": rect(v.frame), "bounds_local": rect(v.bounds),
            "bounds_host": rect(v.convert(v.bounds, to: host)), "alignment_local": rect(v.alignmentRect(forFrame: v.bounds)),
            "alignmentInsets": [ins.top, ins.left, ins.bottom, ins.right].map(Double.init),
            "intrinsic": size(v.intrinsicContentSize), "fitting": size(v.fittingSize), "flipped": v.isFlipped,
            "firstBaselineFromTop": Double(v.firstBaselineOffsetFromTop), "lastBaselineFromBottom": Double(v.lastBaselineOffsetFromBottom),
            "appearance": v.effectiveAppearance.name.rawValue, "hidden": v.isHidden,
            "objectIdentity": String(describing: ObjectIdentifier(v)),
            "bounds_backing": rect(v.convertToBacking(v.bounds)),
            "axLabel": v.accessibilityLabel() ?? "notExposed",
            "axRole": v.accessibilityRole()?.rawValue ?? "notExposed", "axID": v.accessibilityIdentifier() ?? "notExposed"]
        if let c = v as? NSControl {
            d["font"] = font(c.font); d["enabled"] = c.isEnabled; d["controlSize"] = c.controlSize.rawValue
        }
        if let b = v as? NSButton {
            d["title"] = b.title; d["state"] = b.state.rawValue
            d["cellIdentity"] = b.cell.map { String(describing: ObjectIdentifier($0)) } ?? "none"
            d["targetIdentity"] = b.target.map { String(describing: ObjectIdentifier($0)) } ?? "none"
            d["action"] = b.action.map(NSStringFromSelector) ?? "none"
            d["imagePosition"] = b.imagePosition.rawValue; d["imageScaling"] = b.imageScaling.rawValue
            d["contentTint"] = color(b.contentTintColor)
            var runs: [[String: Any]] = []
            b.attributedTitle.enumerateAttributes(in: NSRange(location: 0, length: b.attributedTitle.length)) { attrs, range, _ in
                runs.append(["range": [range.location, range.length], "attributes": attrs.map { key, val in
                    ["key": key.rawValue, "description": String(describing: val)]
                }])
            }
            d["titleAttributeRuns"] = runs
            if let cell = b.cell as? NSButtonCell {
                d["nativeCellState"] = ["highlightsBy": cell.highlightsBy.rawValue, "showsStateBy": cell.showsStateBy.rawValue,
                    "bezelStyle": cell.bezelStyle.rawValue, "bordered": cell.isBordered, "transparent": cell.isTransparent,
                    "focusRingType": cell.focusRingType.rawValue, "lineBreakMode": cell.lineBreakMode.rawValue,
                    "wraps": cell.wraps, "truncatesLastVisibleLine": cell.truncatesLastVisibleLine,
                    "controlSize": cell.controlSize.rawValue, "alignment": cell.alignment.rawValue,
                    "backgroundStyle": cell.backgroundStyle.rawValue, "interiorBackgroundStyle": cell.interiorBackgroundStyle.rawValue,
                    "image": cell.image.map { ["description": $0.description, "size": size($0.size), "template": $0.isTemplate] as [String: Any] } ?? [:],
                    "alternateImage": cell.alternateImage.map { ["description": $0.description, "size": size($0.size), "template": $0.isTemplate] as [String: Any] } ?? [:]] as [String: Any]
            }
            if let cell = b.cell {
                let title = cell.titleRect(forBounds: b.bounds), image = cell.imageRect(forBounds: b.bounds)
                d["cell"] = ["class": NSStringFromClass(type(of: cell)), "size": size(cell.cellSize),
                    "title_local": rect(title), "image_local": rect(image), "drawing_local": rect(cell.drawingRect(forBounds: b.bounds)),
                    "title_host": rect(b.convert(title, to: host)), "image_host": rect(b.convert(image, to: host))]
            }
        }
        // alignment_local above is legacy metadata, retained for compatibility.
        // The native alignment rect takes a parent-coordinate frame, not local bounds.
        let alignmentParent = v.alignmentRect(forFrame: v.frame)
        d["alignment_parent"] = rect(alignmentParent)
        if let superview = v.superview {
            d["alignment_host"] = rect(host.convert(alignmentParent, from: superview))
        }
        d["children"] = v.subviews.enumerated().map { metadata($0.element, host: host, path: path + "." + String($0.offset)) }
        return d
    }
    /// Observational only: strictly the subtree of each owned NestedContainer; never mutates.
    private func nestedProbe(_ host: NSView) -> [String: Any] {
        let containers = views(host).compactMap { $0 as? NestedContainer }
        var groups: [[String: Any]] = []
        var allButtons: [NSButton] = []
        for c in containers {
            let buttons = views(c.hostingView).compactMap { $0 as? NSButton }
            allButtons += buttons
            var entries: [[String: Any]] = []
            for (i, b) in buttons.enumerated() {
                var e = metadata(b, host: host, path: (c.identifier?.rawValue ?? "") + "#" + String(i))
                let cellClass = b.cell.map { NSStringFromClass(type(of: $0)) } ?? "none"
                e["order"] = i; e["cellClass"] = cellClass
                e["radioRoleObserved"] = b.accessibilityRole() == .radioButton
                e["inOwnedScope"] = b.isDescendant(of: c)
                e["targetPresent"] = b.target != nil; e["actionPresent"] = b.action != nil
                e["targetClass"] = b.target.map { String(describing: type(of: $0)) } ?? "none"
                entries.append(e)
            }
            let ideal = c.hostingView.intrinsicContentSize
            groups.append(["container": c.identifier?.rawValue ?? "",
                "containerObject": String(describing: ObjectIdentifier(c)),
                "containerFrame_host": rect(c.convert(c.bounds, to: host)),
                "containerIntrinsic": size(c.intrinsicContentSize), "containerFitting": size(c.fittingSize),
                "hostingObject": String(describing: ObjectIdentifier(c.hostingView)),
                "hostingFrame_parent": rect(c.hostingView.frame),
                "hostingIntrinsic": size(c.hostingView.intrinsicContentSize),
                "hostingFitting": size(c.hostingView.fittingSize),
                "hostingIdealSize": [Double(ideal.width), Double(ideal.height)],
                "hostingFillsBounds": c.hostingView.frame == c.bounds,
                "hostingIsDescendant": c.hostingView.isDescendant(of: host),
                "buttonCount": buttons.count,
                "radioRoleCount": entries.filter { ($0["radioRoleObserved"] as? Bool) == true }.count,
                "selectedCount": buttons.filter { $0.state == .on }.count, "buttons": entries])
        }
        return ["containerCount": containers.count, "totalButtons": allButtons.count,
            "totalSelected": allButtons.filter { $0.state == .on }.count,
            "allTargetsPresent": allButtons.allSatisfy { $0.target != nil },
            "allActionsPresent": allButtons.allSatisfy { $0.action != nil },
            "buttonIdentities": allButtons.map { String(describing: ObjectIdentifier($0)) },
            "buttonTargets": allButtons.map { $0.target.map { String(describing: ObjectIdentifier($0)) } ?? "none" },
            "buttonActions": allButtons.map { $0.action.map(NSStringFromSelector) ?? "none" },
            "buttonStates": allButtons.map { $0.state.rawValue },
            "groups": groups]
    }
    private func views(_ v: NSView) -> [NSView] { [v] + v.subviews.flatMap { views($0) } }
    private func capture(_ host: NSView) throws -> (Data, NSBitmapImageRep) {
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        return (try XCTUnwrap(bitmap.representation(using: .png, properties: [:])), bitmap)
    }
    private func settled(_ host: NSView, window: NSWindow, key: String) throws -> (Data, NSBitmapImageRep) {
        let start = Date(); var previous: Data?; var count = 0
        while Date().timeIntervalSince(start) < 2 {
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            host.layoutSubtreeIfNeeded(); window.display()
            let current = try capture(host)
            count = current.0 == previous ? count + 1 : 1; previous = current.0
            if count >= 3 && Date().timeIntervalSince(start) >= 0.2 { return current }
        }
        throw XCTUnwrapError.unstable(key)
    }
    private enum XCTUnwrapError: Error { case unstable(String) }
    @MainActor
    private struct ExistingRadio: NSViewRepresentable {
        let button: NSButton
        func makeNSView(context: Context) -> NSButton { button }
        func updateNSView(_ view: NSButton, context: Context) {} // Never configure or replace.
        func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSButton, context: Context) -> CGSize? {
            CGSize(width: 16, height: 16)
        }
    }
    @MainActor
    private struct StructuralRows: View {
        let buttons: [NSButton]
        let choices: [CogletRadioGroup.Choice]
        let baselines: [CGFloat]
        let treatment: Treatment
        let pointSize: CGFloat, ink: Color
        @ViewBuilder private func radio(_ index: Int, choiceID: String) -> some View {
            if treatment.centered {
                // Center is independent of the sampled native baseline: no alignmentGuide.
                ExistingRadio(button: buttons[index]).frame(width: 16, height: 16)
                    .background(Marker(name: "button." + choiceID))
            } else {
                let baseline = baselines[index]
                ExistingRadio(button: buttons[index]).frame(width: 16, height: 16)
                    .alignmentGuide(.firstTextBaseline) { _ in baseline }
                    .background(Marker(name: "button." + choiceID))
            }
        }
        var body: some View {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(choices.enumerated()), id: \.element.id) { index, choice in
                    // Factorial hypotheses: native baseline vs center, default gap vs
                    // the Picker's measured 5-point gap (not a fitted pixel shift).
                    HStack(alignment: treatment.centered ? .center : .firstTextBaseline,
                           spacing: treatment.gap5 ? 5 : nil) {
                        radio(index, choiceID: choice.id)
                        Text(choice.label).font(.system(size: pointSize)).foregroundStyle(ink)
                            .fixedSize().accessibilityHidden(true)
                            .background(Marker(name: "label." + choice.id))
                    }
                }
            }
        }
    }
    private func install(_ group: CogletRadioGroup.NativeView, choices: [CogletRadioGroup.Choice],
                         treatment: Treatment, pointSize: CGFloat, ink: Color) throws -> NSHostingView<StructuralRows> {
        XCTAssertEqual(group.buttons.map { $0.accessibilityIdentifier() }, choices.map { Optional($0.id) })
        for button in group.buttons {
            button.title = ""; button.imagePosition = .imageOnly; button.font = .systemFont(ofSize: 13)
            group.removeArrangedSubview(button); button.removeFromSuperview()
            // Sample the baseline for the intended native frame, not the old titled frame.
            button.setFrameSize(NSSize(width: 16, height: 16))
        }
        // Native baseline measured AFTER fixed font AND frame, never fitted to reference pixels.
        let baselines = group.buttons.map { $0.firstBaselineOffsetFromTop }
        let hosting = NSHostingView(rootView: StructuralRows(buttons: group.buttons, choices: choices,
            baselines: baselines, treatment: treatment, pointSize: pointSize, ink: ink))
        group.addArrangedSubview(hosting)
        group.setAccessibilityChildren(group.buttons)
        return hosting
    }
    // Mutable action probe is deliberately separate from the retained 36-case static probe.
    @MainActor
    private final class ActionModel: ObservableObject {
        @Published private(set) var appearance = "System"
        @Published private(set) var scale = 1.0
        @Published private(set) var buttonCount = 0
        private(set) var appearanceWrites = 0
        private(set) var scaleWrites = 0
        var appearanceBinding: Binding<String> {
            Binding(get: { self.appearance }, set: { self.appearanceWrites += 1; self.appearance = $0 })
        }
        var scaleBinding: Binding<Double> {
            Binding(get: { self.scale }, set: { self.scaleWrites += 1; self.scale = $0 })
        }
        func buttonPressed() { buttonCount += 1 }
        var observation: [String: Any] {
            ["appearance": appearance, "scale": scale, "appearanceWrites": appearanceWrites,
             "scaleWrites": scaleWrites, "buttonCount": buttonCount]
        }
    }
    @MainActor
    private struct MutablePicker: View {
        @ObservedObject var model: ActionModel
        let appearance: Bool
        var body: some View {
            if appearance {
                Picker("Appearance", selection: model.appearanceBinding) {
                    ForEach(RadioPresentationMeasurementTests.appearanceChoices, id: \.value) { choice in
                        Text(choice.label).accessibilityIdentifier(choice.id).tag(choice.value)
                    }
                }.pickerStyle(.radioGroup).labelsHidden()
                    .accessibilityIdentifier("settings.general.appearance")
            } else {
                Picker("Text size", selection: model.scaleBinding) {
                    Text("Standard").accessibilityIdentifier("settings.general.text_size.standard").tag(1.0)
                    Text("Large").accessibilityIdentifier("settings.general.text_size.large").tag(1.3)
                    Text("Largest").accessibilityIdentifier("settings.general.text_size.largest").tag(1.6)
                }.pickerStyle(.radioGroup).labelsHidden()
                    .accessibilityIdentifier("settings.general.text_size")
            }
        }
    }
    @MainActor
    private struct MutableFixture: View {
        @ObservedObject var model: ActionModel
        let nested: Bool, disabled: Bool
        // Use the static fixture's exact tokens; no sizing/presentation interventions.
        let palette: Fixture
        private func picker(_ appearance: Bool) -> some View {
            MutablePicker(model: model, appearance: appearance).disabled(disabled)
        }
        private func nestedRoot(_ appearance: Bool) -> AnyView {
            AnyView(picker(appearance).font(.system(size: 16)).foregroundColor(palette.ink)
                .foregroundStyle(palette.ink).tint(palette.accent).environment(\.colorScheme, .light))
        }
        var body: some View {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Appearance").font(.system(size: 16)).accessibilityHidden(true)
                    if nested {
                        NestedHost(name: "appearance", root: nestedRoot(true))
                            .background(Marker(name: "appearance.rows"))
                    } else { picker(true).background(Marker(name: "appearance.rows")) }
                }.background(Marker(name: "appearance.group"))
                VStack(alignment: .leading, spacing: 4) {
                    Text("Text size").font(.system(size: 16)).accessibilityHidden(true)
                    if nested {
                        NestedHost(name: "text_size", root: nestedRoot(false))
                            .background(Marker(name: "text_size.rows"))
                    } else { picker(false).background(Marker(name: "text_size.rows")) }
                }.background(Marker(name: "text_size.group"))
                Text("System Standard").background(Marker(name: "reference.text"))
            }
            .foregroundColor(palette.ink).frame(maxWidth: .infinity, alignment: .leading).padding(16)
            .background(palette.surface, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(palette.border, lineWidth: 1))
            .padding(16).frame(width: 540, height: 440, alignment: .topLeading)
            .font(.system(size: 16)).foregroundStyle(palette.ink)
            .tint(palette.accent).background(palette.background).environment(\.colorScheme, .light)
        }
    }
    @MainActor
    private struct ActionButtonFixture: View {
        @ObservedObject var model: ActionModel
        var body: some View {
            Button("Increment diagnostic counter \(model.buttonCount)") { model.buttonPressed() }
                .accessibilityIdentifier("measurement.positive.button")
                .frame(width: 540, height: 440)
        }
    }
    private struct ActionAXNode {
        let element: (any NSAccessibilityProtocol)?
        let data: [String: Any]
    }
    private struct ActionAXTree {
        let nodes: [ActionAXNode]
        let limits: [String]
        let inspected: Int
        var observation: [String: Any] {
            ["nodes": nodes.map(\.data), "count": nodes.count, "limits": limits,
             "inspectedIncludingRevisits": inspected, "visitedCount": nodes.count,
             "radioIDPlacement": nodes.filter {
                 ($0.data["role"] as? String) == NSAccessibility.Role.radioButton.rawValue
             }.map { radio -> [String: Any] in
                 let path = radio.data["path"] as? String ?? ""
                 return ["radioID": radio.data["id"] ?? "notExposed", "path": path,
                     "children": nodes.filter {
                         ($0.data["path"] as? String)?.hasPrefix(path + ".") == true
                     }.map(\.data)]
             },
             "maxDepth": 8, "maxNodes": 512, "scope": "supplied exact owned host or its exact owning window accessibilityChildren only"]
        }
    }
    // Public protocol verified against Apple NSAccessibilityProtocol documentation:
    // accessibilityChildren() -> [Any]?, accessibilityPerformPress() -> Bool,
    // isAccessibilitySelectorAllowed(_:) -> Bool. No guessed action protocol or BOOL perform().
    // https://developer.apple.com/documentation/appkit/nsaccessibilityprotocol
    private func actionAXTree(_ host: AnyObject) -> ActionAXTree {
        var nodes: [ActionAXNode] = [], limits: [String] = []
        var visited = Set<ObjectIdentifier>()
        var inspected = 0
        let press = #selector(NSAccessibilityProtocol.accessibilityPerformPress)
        func walk(_ object: AnyObject, depth: Int, path: String) {
            guard inspected < 512 else { limits.append("nodeLimit:" + path); return }
            inspected += 1
            guard visited.insert(ObjectIdentifier(object)).inserted else { return }
            let ax = object as? any NSAccessibilityProtocol
            var row: [String: Any] = ["order": nodes.count, "path": path, "depth": depth,
                "objectIdentity": String(describing: ObjectIdentifier(object)),
                "isNSView": object is NSView, "protocolConformance": ax != nil,
                "role": "notExposed", "label": "notExposed", "id": "notExposed",
                "value": "notExposed", "enabled": "notExposed", "frame": [],
                "respondsToPress": false, "pressAllowed": false, "childCount": 0]
            var children: [Any] = []
            if let ax {
                children = ax.accessibilityChildren() ?? []
                row["role"] = ax.accessibilityRole()?.rawValue ?? "notExposed"
                row["label"] = ax.accessibilityLabel() ?? "notExposed"
                row["id"] = ax.accessibilityIdentifier() ?? "notExposed"
                row["value"] = ax.accessibilityValue().map { String(describing: $0) } ?? "notExposed"
                row["enabled"] = ax.isAccessibilityEnabled()
                // Public AX can expose null/infinite rectangles; keep JSON writable.
                row["frame"] = rect(ax.accessibilityFrame()).map { value -> Any in
                    value.isFinite ? value as Any : String(describing: value)
                }
                row["respondsToPress"] = ax.responds(to: press)
                row["pressAllowed"] = ax.isAccessibilitySelectorAllowed(press)
                row["childCount"] = children.count
            }
            if let button = object as? NSButton {
                row["nativeSelected"] = button.state.rawValue
                // Nil target and nil selector are independent observations, not an action verdict.
                row["targetPresent"] = button.target != nil
                row["selectorPresent"] = button.action != nil
                row["selector"] = button.action.map(NSStringFromSelector) ?? "none"
            }
            nodes.append(ActionAXNode(element: ax, data: row))
            guard depth < 8 else {
                if !children.isEmpty { limits.append("depthLimit:" + path) }; return
            }
            for (i, child) in children.enumerated() {
                guard inspected < 512 else { limits.append("nodeLimit:" + path); break }
                walk(child as AnyObject, depth: depth + 1, path: path + "." + String(i))
            }
        }
        walk(host, depth: 0, path: "root")
        return ActionAXTree(nodes: nodes, limits: limits, inspected: inspected)
    }
    /// Fresh synchronous query; exact stable ID only, no cached node, label/frame/class fallback.
    private func pressActionID(_ id: String, host: NSView) -> [String: Any] {
        let fresh = actionAXTree(host)
        let matches = fresh.nodes.filter { ($0.data["id"] as? String) == id }
        let actionable = matches.filter {
            ($0.data["respondsToPress"] as? Bool) == true && ($0.data["pressAllowed"] as? Bool) == true
        }
        let resolution: String
        if !fresh.limits.isEmpty { resolution = "treeunmaterialized" }
        else if matches.isEmpty { resolution = "ax-id-not-exposed" }
        else if matches.count != 1 { resolution = "harnessinvalid" }
        else if matches[0].element == nil { resolution = "ax-node-nonconforming" }
        else if actionable.count != 1 { resolution = "nonactionable" }
        else { resolution = "resolved" }
        var result: [String: Any] = ["requestedID": id, "freshTree": fresh.observation,
            "idMatches": matches.map(\.data), "actionableMatchCount": actionable.count,
            "resolution": resolution, "attempted": false, "returned": false]
        guard resolution == "resolved", let target = actionable[0].element else { return result }
        result["actedNode"] = actionable[0].data
        result["attempted"] = true
        result["returned"] = target.accessibilityPerformPress()
        return result
    }
    private enum ActionCaptureError: Error { case noBitmap, noPNG, unstable(String) }
    // Same cache-display technique as capture(), but no XCTUnwrap assertion before evidence is saved.
    private func actionCapture(_ host: NSView) throws -> (Data, NSBitmapImageRep) {
        guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { throw ActionCaptureError.noBitmap }
        host.cacheDisplay(in: host.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { throw ActionCaptureError.noPNG }
        return (png, bitmap)
    }
    private func actionSettled(_ host: NSView, window: NSWindow, key: String) throws {
        let start = Date(); var previous: Data?; var count = 0
        while Date().timeIntervalSince(start) < 2 {
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            host.layoutSubtreeIfNeeded(); window.display()
            let current = try actionCapture(host).0
            count = current == previous ? count + 1 : 1; previous = current
            if count >= 3 && Date().timeIntervalSince(start) >= 0.2 { return }
        }
        throw ActionCaptureError.unstable(key)
    }
    private func makeActionCase(_ key: String) -> (key: String, model: ActionModel, host: NSHostingView<AnyView>, window: NSWindow) {
        let model = ActionModel()
        let palette = Fixture(treatment: .pickerOriginal, dark: false, pointSize: 16,
            registry: ControlRegistry(revision: 0, currentRoute: { _ in "general" }))
        let content: AnyView = key == "positive-button"
            ? AnyView(ActionButtonFixture(model: model))
            : AnyView(MutableFixture(model: model, nested: key == "nested-original",
                disabled: key == "disabled-picker", palette: palette))
        let host = NSHostingView(rootView: content)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 440),
            styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .aqua); window.isReleasedWhenClosed = false
        window.contentView = host; host.frame = NSRect(x: 0, y: 0, width: 540, height: 440)
        window.orderBack(nil); window.makeFirstResponder(nil)
        return (key, model, host, window)
    }
    // Read-only materialization gate after run33's independent Button tree was empty.
    // No Picker, AX press, synthetic input, system AX client or permission changes.
    func testButtonAXMaterializationOnly() throws {
        guard let path = ProcessInfo.processInfo.environment["SOFTMAX_RADIO_MEASUREMENT_DIR"] else {
            throw XCTSkip("Opt-in Button AX substrate diagnostic only")
        }
        _ = NSApplication.shared
        let root = URL(fileURLWithPath: path, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let item = makeActionCase("positive-button")
        defer {
            item.window.endEditing(for: nil); item.window.makeFirstResponder(nil)
            item.window.contentView = nil; item.window.close()
        }
        var records: [[String: Any]] = []
        var captureErrors: [String] = []
        let expectedID = "measurement.positive.button"
        func checkpoint(_ phase: String, hit: AnyObject? = nil) throws {
            do { try actionSettled(item.host, window: item.window, key: phase) }
            catch { captureErrors.append(phase + ": " + String(describing: error)) }
            let (png, bitmap) = try actionCapture(item.host)
            let hostTree = actionAXTree(item.host)
            let windowTree = actionAXTree(item.window)
            let name = "button-materialization-" + phase
            try png.write(to: root.appendingPathComponent(name + ".png"))
            // A hit-test result is observed separately, NEVER used for action routing.
            let hitTree = hit.map { actionAXTree($0) }
            let row: [String: Any] = ["phase": phase, "hostAX": hostTree.observation,
                "windowAX": windowTree.observation,
                "hitAX": hitTree?.observation as Any? ?? "notQueried",
                "hitExactIDCount": hitTree?.nodes.filter { ($0.data["id"] as? String) == expectedID }.count ?? 0,
                "limits": hostTree.limits + windowTree.limits + (hitTree?.limits ?? []),
                "hostExactIDCount": hostTree.nodes.filter { ($0.data["id"] as? String) == expectedID }.count,
                "windowExactIDCount": windowTree.nodes.filter { ($0.data["id"] as? String) == expectedID }.count,
                "model": item.model.observation, "windowKey": item.window.isKeyWindow,
                "appActive": NSApp.isActive, "styleMask": item.window.styleMask.rawValue,
                "hostFrame": rect(item.host.frame), "hostBounds": rect(item.host.bounds),
                "backingScale": Double(item.window.backingScaleFactor),
                "rasterSize": [bitmap.pixelsWide, bitmap.pixelsHigh], "png": name + ".png",
                "nativeTree": metadata(item.host, host: item.host, path: "root")]
            records.append(row)
            try JSONSerialization.data(withJSONObject: row, options: [.prettyPrinted, .sortedKeys])
                .write(to: root.appendingPathComponent(name + ".json"))
        }
        try checkpoint("borderless-back")
        item.window.orderFront(nil)
        try checkpoint("borderless-front")
        // Diagnostic presentation change only: borderless windows may refuse key status.
        item.window.styleMask.insert(.titled)
        item.window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        try checkpoint("titled-key-request")
        let center = NSPoint(x: item.host.bounds.midX, y: item.host.bounds.midY)
        let screenPoint = item.window.convertPoint(toScreen: item.host.convert(center, to: nil))
        let hit = item.host.accessibilityHitTest(screenPoint) as AnyObject?
        try checkpoint("after-owned-host-hit-test", hit: hit)
        let idCounts = records.flatMap { row in
            ["hostExactIDCount", "windowExactIDCount", "hitExactIDCount"].map { row[$0] as? Int ?? 0 }
        }
        let exposed = idCounts.contains(1)
        let inconclusive = !captureErrors.isEmpty || records.count != 4 || idCounts.contains { $0 > 1 }
            || records.contains { !(($0["limits"] as? [String]) ?? []).isEmpty }
        let outcome = inconclusive ? "inconclusive-limits-duplicates-or-capture"
            : exposed ? "exact-button-id-exposed" : "in-process-AX-route-unavailable"
        let result: [String: Any] = ["source": ProcessInfo.processInfo.environment["GITHUB_SHA"] ?? "unrecorded",
            "cases": records, "outcome": outcome, "captureErrors": captureErrors,
            "meaning": "read-only feasibility ONLY; no Picker or action inference; negative closes this tested route",
            "noActionsAttempted": true, "model": item.model.observation,
            "scope": "one exact owned host and its exact owning window; no app/global window enumeration",
            "qualification": "not product, stable-id census, visual, focus, lifecycle or shared-command acceptance"]
        try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            .write(to: root.appendingPathComponent("button-materialization-cases.json"))
        // Exposure may legitimately remain absent. A successful run means evidence retained.
        XCTAssertEqual(records.count, 4)
        XCTAssertTrue(captureErrors.isEmpty, captureErrors.joined(separator: "\n"))
        XCTAssertEqual(item.model.buttonCount, 0)
        XCTAssertEqual(item.model.appearanceWrites, 0)
        XCTAssertEqual(item.model.scaleWrites, 0)
    }
    func testMutablePickerActionSubstrateOnly() throws {
        guard let path = ProcessInfo.processInfo.environment["SOFTMAX_RADIO_MEASUREMENT_DIR"] else {
            throw XCTSkip("Opt-in mutable action substrate diagnostic only")
        }
        _ = NSApplication.shared
        let root = URL(fileURLWithPath: path, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var failures: [String] = []
        var records: [[String: Any]] = []
        var buttonValid = false
        var disabledValid = false
        func writeJSON(_ value: [String: Any], _ name: String) throws {
            try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
                .write(to: root.appendingPathComponent(name + ".json"))
        }
        // Button is an independent host; disabled Picker has an independent, retained model.
        // No synthetic mouse phase: bounded native tracking has not been established/reviewed.
        let controlOrder = ["positive-button", "disabled-picker", "direct-original", "nested-original"]
        precondition(controlOrder.prefix(2).elementsEqual(["positive-button", "disabled-picker"]),
            "Positive and disabled controls must precede all Picker result classification")
        let ownedCases = controlOrder.map { key in makeActionCase(key) }
        defer {
            for item in ownedCases {
                item.window.endEditing(for: nil); item.window.makeFirstResponder(nil)
                item.window.contentView = nil; item.window.close()
            }
        }
        // BOTH retained original hosts are rasterized before ANY action (including controls).
        for item in ownedCases where item.key == "direct-original" || item.key == "nested-original" {
            let name = "mutable-light-16.0-" + item.key + "-retained-unacted"
            do {
                try actionSettled(item.host, window: item.window, key: name)
                let (png, bitmap) = try actionCapture(item.host)
                try png.write(to: root.appendingPathComponent(name + ".png"))
                try writeJSON(["model": item.model.observation,
                    "tree": metadata(item.host, host: item.host, path: "root"),
                    "backingScale": Double(item.window.backingScaleFactor),
                    "rasterSize": [bitmap.pixelsWide, bitmap.pixelsHigh], "png": name + ".png",
                    "source": ProcessInfo.processInfo.environment["GITHUB_SHA"] ?? "unrecorded"], name)
            } catch { failures.append(name + ": " + String(describing: error)) }
        }
        for item in ownedCases {
            let (key, model, host, window) = (item.key, item.model, item.host, item.window)
            let prefix = "mutable-light-16.0-" + key
            var checkpoints: [[String: Any]] = [], actions: [[String: Any]] = []
            var previousPNG: Data?
            func checkpoint(_ phase: String) throws -> [String: Any] {
                var stable = true
                do { try actionSettled(host, window: window, key: prefix + "-" + phase) }
                catch { stable = false; failures.append(prefix + "-" + phase + ": " + String(describing: error)) }
                let (png, bitmap) = try actionCapture(host)
                let name = prefix + "-" + phase
                try png.write(to: root.appendingPathComponent(name + ".png"))
                let ax = actionAXTree(host)
                let native = views(host).compactMap { $0 as? NSButton }
                let value: [String: Any] = ["phase": phase, "stable": stable, "model": model.observation,
                    "ax": ax.observation, "tree": metadata(host, host: host, path: "root"),
                    "nativeButtons": native.map { metadata($0, host: host, path: $0.accessibilityIdentifier() ?? "") },
                    "nativeSelectedCount": native.filter { $0.state == .on }.count,
                    "nativeRawStates": native.map { button -> [String: Any] in
                        ["objectIdentity": String(describing: ObjectIdentifier(button)),
                         "id": button.accessibilityIdentifier() ?? "notExposed", "state": button.state.rawValue,
                         "axValue": button.accessibilityValue().map { String(describing: $0) } ?? "notExposed"]
                    },
                    "png": name + ".png", "rasterChangedSincePrevious": previousPNG.map { $0 != png } ?? false,
                    "backingScale": Double(window.backingScaleFactor), "rasterSize": [bitmap.pixelsWide, bitmap.pixelsHigh],
                    "hostSize": size(host.bounds.size), "colorSpace": bitmap.colorSpace.description,
                    "windowKey": window.isKeyWindow, "appActive": NSApp.isActive,
                    "firstResponder": window.firstResponder.map { String(describing: type(of: $0)) } ?? "none"]
                previousPNG = png
                try writeJSON(value, name)
                checkpoints.append(value)
                return value
            }
            do {
                // Retain unacted direct/nested rasters before any action on that host.
                _ = try checkpoint("unacted")
                let ids = key == "positive-button" ? ["measurement.positive.button"] : [
                    "settings.general.appearance.light", "settings.general.text_size.large"]
                for (index, id) in ids.enumerated() {
                    let before = model.observation
                    let beforePNG = try actionCapture(host).0
                    let pressStart = Date()
                    var action = pressActionID(id, host: host)
                    action["modelBefore"] = before
                    action["modelImmediatelyAfter"] = model.observation
                    // Preserve the attempted action even if polling/readback later fails.
                    actions.append(action)
                    try writeJSON(action, prefix + "-action-" + String(index))
                    var modelChangeAt: Double?, rasterChangeAt: Double?
                    let pollStart = Date()
                    repeat {
                        host.layoutSubtreeIfNeeded(); window.display()
                        let elapsed = Date().timeIntervalSince(pressStart)
                        if modelChangeAt == nil && (model.appearance != (before["appearance"] as? String)
                            || model.scale != (before["scale"] as? Double)
                            || model.buttonCount != (before["buttonCount"] as? Int)) { modelChangeAt = elapsed }
                        let currentPNG = try actionCapture(host).0
                        if rasterChangeAt == nil && currentPNG != beforePNG { rasterChangeAt = elapsed }
                        if modelChangeAt != nil || rasterChangeAt != nil { break }
                        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
                    } while Date().timeIntervalSince(pollStart) < 1.0
                    action["pollElapsedSeconds"] = Date().timeIntervalSince(pollStart)
                    action["firstModelChangeSeconds"] = modelChangeAt.map { $0 as Any } ?? "notObserved"
                    action["firstRasterChangeSeconds"] = rasterChangeAt.map { $0 as Any } ?? "notObserved"
                    action["pollStopRule"] = "first model OR raster change, otherwise full 1s; then stable fresh readback"
                    let after = try checkpoint("after-press-" + String(index))
                    action["readback"] = after
                    let fresh = actionAXTree(host)
                    let expected = [model.appearance == "Light" ? "settings.general.appearance.light" : "settings.general.appearance.system",
                        model.scale == 1.3 ? "settings.general.text_size.large" : "settings.general.text_size.standard"]
                    let exactValues = (Self.appearanceChoices + Self.sizeChoices).allSatisfy { choice in
                        let matches = fresh.nodes.filter { ($0.data["id"] as? String) == choice.id }
                        return matches.count == 1 && matches[0].element != nil
                            && (matches[0].data["role"] as? String) == NSAccessibility.Role.radioButton.rawValue
                            && (matches[0].data["value"] as? String) == (expected.contains(choice.id) ? "1" : "0")
                    }
                    action["freshAXValueTree"] = fresh.observation
                    action["freshExactRadioValuesQualified"] = fresh.limits.isEmpty && exactValues
                    if key != "positive-button" {
                        action["modelTransitionQualified"] = index == 0
                            ? model.appearance == "Light" && model.appearance != (before["appearance"] as? String)
                                && model.appearanceWrites > (before["appearanceWrites"] as? Int ?? 0)
                                && model.scale == (before["scale"] as? Double)
                            : model.scale == 1.3 && model.scale != (before["scale"] as? Double)
                                && model.scaleWrites > (before["scaleWrites"] as? Int ?? 0)
                                && model.appearance == (before["appearance"] as? String)
                    }
                    actions[index] = action
                    try writeJSON(action, prefix + "-action-" + String(index))
                }
            } catch {
                failures.append(prefix + ": " + String(describing: error))
            }
            let stable = checkpoints.count == (key == "positive-button" ? 2 : 3)
                && checkpoints.allSatisfy { ($0["stable"] as? Bool) == true }
            let complete = !checkpoints.isEmpty && checkpoints.allSatisfy {
                let ax = $0["ax"] as? [String: Any]
                return (ax?["limits"] as? [String])?.isEmpty == true
            }
            let finalTree = actionAXTree(host)
            let radios = finalTree.nodes.filter { ($0.data["role"] as? String) == NSAccessibility.Role.radioButton.rawValue }
            let initialNative = (checkpoints.first?["nativeButtons"] as? [[String: Any]]) ?? []
            let finalNative = views(host).compactMap { $0 as? NSButton }
            let changed = checkpoints.dropFirst().allSatisfy { ($0["rasterChangedSincePrevious"] as? Bool) == true }
            let pressed = actions.count == (key == "positive-button" ? 1 : 2)
                && actions.allSatisfy { ($0["attempted"] as? Bool) == true && ($0["returned"] as? Bool) == true }
            let initialRadios = ((checkpoints.first?["ax"] as? [String: Any])?["nodes"] as? [[String: Any]] ?? [])
                .filter { ($0["role"] as? String) == NSAccessibility.Role.radioButton.rawValue }
            let treeMaterialized = complete && initialRadios.count == 6 && radios.count == 6
            let unchanged = checkpoints.count == 3 && checkpoints.dropFirst().allSatisfy {
                ($0["rasterChangedSincePrevious"] as? Bool) == false
            }
            let disabledActionEvidence = actions.count == 2 && actions.allSatisfy { action in
                let matches = action["idMatches"] as? [[String: Any]] ?? []
                let resolved = matches.count == 1 && (matches[0]["protocolConformance"] as? Bool) == true
                    && (matches[0]["role"] as? String) == NSAccessibility.Role.radioButton.rawValue
                let nonactionable = (action["resolution"] as? String) == "nonactionable"
                let refused = (action["attempted"] as? Bool) == true && (action["returned"] as? Bool) == false
                return resolved && (nonactionable || refused)
                    && (action["pollElapsedSeconds"] as? Double ?? 0) >= 1.0
            }
            if key == "positive-button" {
                buttonValid = stable && complete && pressed && changed && model.buttonCount == 1
                    && model.appearanceWrites == 0 && model.scaleWrites == 0
            } else if key == "disabled-picker" {
                disabledValid = stable && treeMaterialized && disabledActionEvidence && model.appearance == "System"
                    && model.scale == 1.0 && model.appearanceWrites == 0 && model.scaleWrites == 0
                    && model.buttonCount == 0 && unchanged
                    && initialNative.count == 6 && finalNative.count == 6
                    && initialNative.map { $0["state"] as? Int } == finalNative.map { Optional($0.state.rawValue) }
            }
            let modelChanged = model.appearance == "Light" && model.scale == 1.3
                && model.appearanceWrites >= 1 && model.scaleWrites >= 1 && model.buttonCount == 0
            // Native/AX state is retained independently; don't use labels or frame to route actions.
            let nativeSelectedIDs = finalNative.filter { $0.state == .on }.compactMap { $0.accessibilityIdentifier() }
            let nativeSelection = finalNative.count == 6
                && Set(finalNative.compactMap { $0.accessibilityIdentifier() })
                    == Set((Self.appearanceChoices + Self.sizeChoices).map(\.id))
                && Set(nativeSelectedIDs) == Set([
                "settings.general.appearance.light", "settings.general.text_size.large"])
                && finalNative.filter { $0.state == .on }.count == 2
                && finalNative.allSatisfy { ($0.accessibilityValue() as? NSNumber)?.intValue == $0.state.rawValue }
            let freshAXValues = actions.count == 2 && actions.allSatisfy {
                ($0["freshExactRadioValuesQualified"] as? Bool) == true
                    && ($0["modelTransitionQualified"] as? Bool) == true
            }
            // Public AX function and native display-button identity are independent gates.
            // Blank display IDs cannot erase qualified public AX action/readback evidence.
            let works = stable && treeMaterialized && pressed && modelChanged && freshAXValues && changed
            let resolutions = actions.compactMap { $0["resolution"] as? String }
            let outcome: String
            if !buttonValid || (key != "positive-button" && key != "disabled-picker" && !disabledValid) || !stable || !complete {
                outcome = "harnessinvalid"
            } else if key == "positive-button" { outcome = "AXworks" }
            else if !treeMaterialized { outcome = "treeunmaterialized" }
            else if key == "disabled-picker" { outcome = disabledValid ? "nonactionable" : "harnessinvalid" }
            else if works { outcome = "AXworks" }
            else if resolutions.contains("treeunmaterialized") || actions.contains(where: {
                let tree = $0["freshAXValueTree"] as? [String: Any]
                return (tree?["limits"] as? [String])?.isEmpty == false
            }) { outcome = "treeunmaterialized" }
            else if resolutions.contains("harnessinvalid") { outcome = "harnessinvalid" }
            else if resolutions.contains("ax-node-nonconforming") { outcome = "ax-node-nonconforming" }
            else if resolutions.contains("ax-id-not-exposed") { outcome = "ax-id-not-exposed" }
            else if resolutions.contains("nonactionable") { outcome = "nonactionable" }
            else { outcome = "ax-no-effect" }
            let record: [String: Any] = ["case": key, "outcome": outcome, "model": model.observation,
                "positiveButtonValid": buttonValid, "disabledPickerValid": disabledValid,
                "stable": stable, "treeMaterialized": treeMaterialized, "actions": actions, "checkpoints": checkpoints,
                "modelTransitionQualified": modelChanged, "freshExactRadioValuesQualifiedEachPress": freshAXValues,
                "publicAXFunctional": works, "nativeSelectionAndAXValue": nativeSelection,
                "nativeSelectedIDs": nativeSelectedIDs, "disabledActionEvidenceQualified": disabledActionEvidence,
                "nativeIdentityGate": nativeSelection,
                "nativeIdentityGateMeaning": "separate six exact native IDs + selected IDs + state/AX values; false does not negate public AX function or waive final census",
                "rasterChangedEachPress": changed, "mouse": "not attempted: bounded tracking unproven",
                "source": ProcessInfo.processInfo.environment["GITHUB_SHA"] ?? "unrecorded",
                "productBase": "bb4949eb4ec4667186478142a999cda76ba69532", "os": ProcessInfo.processInfo.operatingSystemVersionString,
                "pointSize": 16, "theme": "light", "treatment": key,
                "offlineGates": ["unacted direct and nested full raster/components/geometry versus retained run32 light16original",
                    "any mismatch is negative; no offsets, tuning, adoption or visual qualification",
                    "static nested14pt negative remains unchanged; arbitrary proposals/lifecycle not qualified",
                    "ADR0030: test-only public AX actions do not authorize an AX-only production adapter"]]
            try writeJSON(record, prefix)
            records.append(record)
            withExtendedLifetime(model) {}
        }
        try writeJSON(["cases": records, "errors": failures, "source": ProcessInfo.processInfo.environment["GITHUB_SHA"] ?? "unrecorded",
            "meaning": "mutable public-AX diagnostic only, NOT visual acceptance or production authorization",
            "outcomes": ["ax-id-not-exposed", "ax-node-nonconforming", "nonactionable", "ax-no-effect",
                "AXworks", "harnessinvalid", "treeunmaterialized"],
            "classificationScope": "public AX only; AXworks requires exact IDs, press, model/raster transitions and fresh radio 0/1 values, not native display IDs; ax-no-effect means no fully qualified effect",
            "offlineFidelityMandatory": true,
            "offlineFidelityStatus": "pending: direct/nested light16 unacted PNGs must pass retained run32 raster/components/geometry comparison; no visual or production qualification here",
            "mousePhase": "omitted pending bounded-tracking plan amendment and independent review"], "mutable-action-cases")
        // Only harness validity is asserted, and only after every available artifact is written.
        XCTAssertTrue(buttonValid, "Positive SwiftUI Button AX control invalid; Picker negatives uninterpretable")
        XCTAssertTrue(disabledValid, "Disabled Picker control invalid; Picker negatives uninterpretable")
        XCTAssertTrue(failures.isEmpty, failures.joined(separator: "\n"))
    }
    func testPairedNativeMeasurementsOnly() throws {
        guard let path = ProcessInfo.processInfo.environment["SOFTMAX_RADIO_MEASUREMENT_DIR"] else {
            throw XCTSkip("Opt-in measurement only, not visual acceptance")
        }
        _ = NSApplication.shared
        let root = URL(fileURLWithPath: path, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var cases: [String] = []
        for dark in [false, true] {
            for pointSize in [CGFloat(14), CGFloat(18.2), CGFloat(22.4), CGFloat(16), CGFloat(20), CGFloat(26)] {
                for treatment in Treatment.probeCases {
                    let owned = treatment.owned
                    let key = "\(dark ? "dark" : "light")-\(pointSize)-\(treatment.rawValue)"
                    let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
                    let fixture = Fixture(treatment: treatment, dark: dark, pointSize: pointSize, registry: registry)
                    let host = NSHostingView(rootView: fixture)
                    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 440),
                        styleMask: [.borderless], backing: .buffered, defer: false)
                    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                    window.isReleasedWhenClosed = false
                    defer { window.endEditing(for: nil); window.makeFirstResponder(nil); window.contentView = nil; window.close() }
                    window.contentView = host; host.frame = NSRect(x: 0, y: 0, width: 540, height: 440)
                    window.orderBack(nil); window.makeFirstResponder(nil)
                    // Mount and settle BEFORE taking graph/mount snapshots or installing anything.
                    _ = try settled(host, window: window, key: key + "-initial")
                    let groups = views(host).compactMap { $0 as? CogletRadioGroup.NativeView }
                    let initialGroups = groups.map(ObjectIdentifier.init)
                    let initialGroupIDs = groups.map { $0.accessibilityIdentifier() }
                    let initialGroupLabels = groups.map { $0.accessibilityLabel() }
                    let nativeButtons = views(host).compactMap { $0 as? NSButton }
                    let initialButtons = nativeButtons.map(ObjectIdentifier.init)
                    let initialTargets = nativeButtons.map { $0.target.map(ObjectIdentifier.init) }
                    let initialActions = nativeButtons.map { $0.action }
                    let initialStates = nativeButtons.map { $0.state }
                    let initialFrames = nativeButtons.map { $0.frame }
                    let initialIDs = nativeButtons.map { $0.accessibilityIdentifier() }
                    let initialLabels = nativeButtons.map { $0.accessibilityLabel() }
                    let initialCells = nativeButtons.map { $0.cell.map(ObjectIdentifier.init) }
                    let initialInventory = registry.registrationInventory()
                    let nestedInitial: [String: Any]? = treatment.nested ? nestedProbe(host) : nil
                    // Inventory includes BOTH parent groups and every child. prepareFocus only reads tokens.
                    let mountIDs = initialInventory.map(\.id)
                    @MainActor func mounts() throws -> [ControlRegistry.Mount] {
                        try mountIDs.map { id in
                            try registry.captureDirectory().prepareFocus(.map([
                                .init("window", .text("settings")), .init("control", .text(id))]))
                        }
                    }
                    let initialMounts = try mounts()
                    let nativeBefore = nativeButtons.map { metadata($0, host: host, path: $0.accessibilityIdentifier() ?? "") }
                    var retainedHosts: [NSHostingView<StructuralRows>] = []
                    if owned {
                        XCTAssertEqual(nativeButtons.count, 6); XCTAssertEqual(groups.count, 2)
                        XCTAssertEqual(initialInventory.count, 8); XCTAssertEqual(initialMounts.count, 8)
                        let before = try capture(host)
                        try before.0.write(to: root.appendingPathComponent(key + "-before.png"))
                        try JSONSerialization.data(withJSONObject: metadata(host, host: host, path: "root"), options: [.prettyPrinted, .sortedKeys])
                            .write(to: root.appendingPathComponent(key + "-before.json"))
                        // Identical setter/display checkpoint for ALL owned cases, no configure/remount.
                        for button in nativeButtons {
                            button.attributedTitle = NSAttributedString(attributedString: button.attributedTitle)
                            button.needsDisplay = true
                        }
                        window.display()
                        let noop = try capture(host)
                        try noop.0.write(to: root.appendingPathComponent(key + "-noop.png"))
                        try JSONSerialization.data(withJSONObject: metadata(host, host: host, path: "root"), options: [.prettyPrinted, .sortedKeys])
                            .write(to: root.appendingPathComponent(key + "-noop.json"))
                        XCTAssertEqual(before.0, noop.0, "Unchanged setter checkpoint: " + key)
                        if treatment.structural {
                            for group in groups {
                                let id = try XCTUnwrap(group.accessibilityIdentifier())
                                let choices: [CogletRadioGroup.Choice]
                                switch id {
                                case "settings.general.appearance": choices = Self.appearanceChoices
                                case "settings.general.text_size": choices = Self.sizeChoices
                                default: XCTFail("Unexpected group: " + id); continue
                                }
                                retainedHosts.append(try install(group, choices: choices, treatment: treatment, pointSize: pointSize, ink: fixture.ink))
                            }
                        }
                    }
                    let stable = try settled(host, window: window, key: key)
                    for button in nativeButtons { button.needsDisplay = true }
                    host.needsDisplay = true
                    let (png, bitmap) = try capture(host)
                    XCTAssertEqual(png, stable.0, "Forced capture changed stable pixels: " + key)
                    // Retain pixels/metadata even if a presentation invariant fails.
                    try png.write(to: root.appendingPathComponent(key + ".png"))
                    let all = views(host)
                    let labelMarkers = all.filter { $0.identifier?.rawValue.hasPrefix("label.") == true }
                    let wrapperMarkers = all.filter { $0.identifier?.rawValue.hasPrefix("button.") == true }
                    XCTAssertEqual(labelMarkers.count, treatment == .pickerMarked || treatment.structural ? 6 : 0)
                    XCTAssertEqual(wrapperMarkers.count, treatment.structural ? 6 : 0)
                    // Correlate structural row geometry by choice identity, never tree order.
                    // All rectangles are host coordinates; rows-local values derive offline
                    // from the existing appearance.rows / text_size.rows markers.
                    var rowGeometry: [[String: Any]] = []
                    if treatment.structural {
                        for choice in Self.appearanceChoices + Self.sizeChoices {
                            let button = try XCTUnwrap(nativeButtons.first { $0.accessibilityIdentifier() == choice.id })
                            let label = try XCTUnwrap(labelMarkers.first { $0.identifier?.rawValue == "label." + choice.id })
                            let wrapper = try XCTUnwrap(wrapperMarkers.first { $0.identifier?.rawValue == "button." + choice.id })
                            rowGeometry.append(["choiceID": choice.id,
                                "buttonBounds_host": rect(button.convert(button.bounds, to: host)),
                                "labelBounds_host": rect(label.convert(label.bounds, to: host)),
                                "wrapperBounds_host": rect(wrapper.convert(wrapper.bounds, to: host))])
                        }
                    }
                    XCTAssertEqual(rowGeometry.count, treatment.structural ? 6 : 0)
                    if owned {
                        let buttons = all.compactMap { $0 as? NSButton }
                        XCTAssertEqual(buttons.count, 6)
                        if treatment.structural {
                            for button in buttons {
                                XCTAssertEqual(button.title, ""); XCTAssertEqual(button.imagePosition, .imageOnly)
                                XCTAssertEqual(button.font, NSFont.systemFont(ofSize: 13))
                                XCTAssertEqual(button.frame.size, NSSize(width: 16, height: 16))
                            }
                            for retained in retainedHosts {
                                XCTAssertEqual(retained.rootView.buttons.map { $0.firstBaselineOffsetFromTop },
                                    retained.rootView.baselines, "Native baseline changed after mounting: " + key)
                            }
                        } else {
                            XCTAssertEqual(Set(buttons.map(\.title)), Set(["System", "Light", "Dark", "Standard", "Large", "Largest"]))
                            XCTAssertTrue(buttons.allSatisfy { $0.font?.pointSize == pointSize })
                        }
                        XCTAssertEqual(buttons.filter { $0.state == .on }.count, 2)
                        XCTAssertEqual(registry.registrationInventory().count, 8)
                    }
                    var crops: [[String: Any]] = []
                    for marker in all where ["appearance.rows", "text_size.rows", "reference.text"].contains(marker.identifier?.rawValue ?? "") {
                        let r = marker.convert(marker.bounds, to: host)
                        XCTAssertGreaterThan(r.width, 0); XCTAssertGreaterThan(r.height, 0)
                        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: r))
                        host.cacheDisplay(in: r, to: rep)
                        let name = key + "-" + marker.identifier!.rawValue + ".png"
                        try XCTUnwrap(rep.representation(using: .png, properties: [:])).write(to: root.appendingPathComponent(name))
                        crops.append(["name": name, "host_rect": rect(r), "pixelSize": [rep.pixelsWide, rep.pixelsHigh]])
                    }
                    XCTAssertEqual(crops.count, 3)
                    var colors: [String: Any] = [:]
                    window.appearance!.performAsCurrentDrawingAppearance {
                        colors = ["labelColor": color(.labelColor), "tokenInk": color(NSColor(srgbRed: dark ? 0.925490196 : 0.090196078,
                            green: dark ? 0.960784314 : 0.137254902, blue: dark ? 0.937254902 : 0.113725490, alpha: 1))]
                    }
                    let nestedFinal: [String: Any]? = treatment.nested ? nestedProbe(host) : nil
                    var nestedEvidence: [String: Any] = [:]
                    if let a = nestedInitial, let b = nestedFinal {
                        func same(_ k: String) -> Bool { String(describing: a[k] ?? "") == String(describing: b[k] ?? "") }
                        let groups = b["groups"] as? [[String: Any]] ?? []
                        let assumptions: [String: Bool] = [
                            "containerCount2": (b["containerCount"] as? Int) == 2,
                            "eachGroup3Buttons": groups.allSatisfy { ($0["buttonCount"] as? Int) == 3 },
                            "totalSelected2": (b["totalSelected"] as? Int) == 2,
                            "targetsActionsPresent": (b["allTargetsPresent"] as? Bool) == true && (b["allActionsPresent"] as? Bool) == true,
                            "identitiesStable": same("buttonIdentities"), "targetsStable": same("buttonTargets"),
                            "actionsStable": same("buttonActions"), "statesStable": same("buttonStates")]
                        nestedEvidence = ["initial": a, "final": b, "assumptions": assumptions,
                            "meaning": "feasibility evidence only; nesting fidelity is judged offline, never asserted"]
                    }
                    let result: [String: Any] = ["case": key, "kind": owned ? "owned" : (treatment.nested ? "nested" : "picker"),
                        "nestedProbe": nestedEvidence, "pointSize": Double(pointSize),
                        "treatment": treatment.rawValue,
                        "offlineGates": ["current picker-original == retained run31 picker-original EXACT across all themes/sizes before interpreting nesting",
                            "full-raster picker-original == picker-marked EXACT before marker geometry is trusted",
                            "nested-original == current picker-original EXACT full scene AND untranslated label/disc components, group/rows/reference geometry",
                            "any mismatch is a valid negative; no shift/offset tuning, no adoption or automatic product qualification",
                            "static ideal-size host ONLY; arbitrary proposals, stable ids, actions, focus, updates and lifecycle UNQUALIFIED"],
                        "scaleCoverage": window.backingScaleFactor == 1 ? "1x observed; 2x UNTESTED" : "only actual backingScale observed",
                        "nativeBefore": nativeBefore,
                        "labelFrames": labelMarkers.map { metadata($0, host: host, path: $0.identifier!.rawValue) },
                        "buttonWrapperFrames": wrapperMarkers.map { metadata($0, host: host, path: $0.identifier!.rawValue) },
                        "rowGeometry": rowGeometry,
                        "nativeAfter": nativeButtons.map { metadata($0, host: host, path: $0.accessibilityIdentifier() ?? "") },
                        "mountIDs": mountIDs, "mountsBefore": initialMounts.map { String(describing: $0) },
                        "mountsAfter": try mounts().map { String(describing: $0) },
                        "source": ProcessInfo.processInfo.environment["GITHUB_SHA"] ?? "unrecorded",
                        "base": "bb4949eb4ec4667186478142a999cda76ba69532", "os": ProcessInfo.processInfo.operatingSystemVersionString,
                        "backingScale": Double(window.backingScaleFactor), "rasterSize": [bitmap.pixelsWide, bitmap.pixelsHigh],
                        "colorSpace": bitmap.colorSpace.description, "windowKey": window.isKeyWindow,
                        "appActive": NSApp.isActive, "firstResponder": window.firstResponder.map { String(describing: type(of: $0)) } ?? "none",
                        "coordinates": "AppKit host points; use flipped plus raster/host ratios for pixels",
                        "colors": colors, "crops": crops, "tree": metadata(host, host: host, path: "root"),
                        "pickerNativeCellGeometry": owned ? "see actual button cells" : "notExposed unless present in tree"]
                    XCTAssertTrue(JSONSerialization.isValidJSONObject(result), "Nonfinite or unsupported metadata: \(key)")
                    try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                        .write(to: root.appendingPathComponent(key + ".json"))
                    cases.append(key)
                    // Asserted only AFTER pixels/metadata are retained; fidelity is never asserted.
                    if treatment.nested, let ev = nestedEvidence["assumptions"] as? [String: Bool] {
                        for (name, ok) in ev.sorted(by: { $0.key < $1.key }) { XCTAssertTrue(ok, "nested probe assumption \(name): " + key) }
                    }
                    if owned {
                        XCTAssertEqual(views(host).compactMap { $0 as? CogletRadioGroup.NativeView }.map(ObjectIdentifier.init), initialGroups)
                        XCTAssertEqual(groups.map { $0.accessibilityIdentifier() }, initialGroupIDs)
                        XCTAssertEqual(groups.map { $0.accessibilityLabel() }, initialGroupLabels)
                        XCTAssertEqual(views(host).compactMap { $0 as? NSButton }.map(ObjectIdentifier.init), initialButtons)
                        XCTAssertEqual(nativeButtons.map { $0.target.map(ObjectIdentifier.init) }, initialTargets)
                        XCTAssertEqual(nativeButtons.map { $0.action }, initialActions)
                        XCTAssertEqual(nativeButtons.map { $0.state }, initialStates)
                        if !treatment.structural { XCTAssertEqual(nativeButtons.map { $0.frame }, initialFrames) }
                        XCTAssertEqual(nativeButtons.map { $0.accessibilityLabel() }, initialLabels)
                        for group in groups {
                            let children = try XCTUnwrap(group.accessibilityChildren() as? [NSButton])
                            XCTAssertEqual(children.map(ObjectIdentifier.init), group.buttons.map(ObjectIdentifier.init))
                        }
                        XCTAssertEqual(nativeButtons.map { $0.accessibilityIdentifier() }, initialIDs)
                        XCTAssertEqual(nativeButtons.map { $0.cell.map(ObjectIdentifier.init) }, initialCells)
                        XCTAssertEqual(registry.registrationInventory(), initialInventory)
                        XCTAssertEqual(try mounts(), initialMounts)
                    }
                    withExtendedLifetime(retainedHosts) {}
                }
            }
        }
        // Three probe treatments × two themes × six sizes; measurement, NOT visual acceptance.
        XCTAssertEqual(cases.count, 36); XCTAssertEqual(Set(cases).count, 36)
        try JSONSerialization.data(withJSONObject: ["cases": cases, "meaning": "measurement complete, NOT visual acceptance"], options: [.prettyPrinted])
            .write(to: root.appendingPathComponent("cases.json"))
    }
}
#endif
