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
    private struct Fixture: View {
        let treatment: Treatment, dark: Bool, pointSize: CGFloat
        var owned: Bool { treatment.owned }
        private var appearanceChoices: [CogletRadioGroup.Choice] { RadioPresentationMeasurementTests.appearanceChoices }
        private var sizeChoices: [CogletRadioGroup.Choice] { RadioPresentationMeasurementTests.sizeChoices }
        let registry: ControlRegistry
        // Immutable bindings: no SwiftUI updates may reconfigure the installed graph.
        private let appearance = Binding.constant("System")
        private let scale = Binding.constant(1.0)
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
                    } else {
                        Picker("Appearance", selection: appearance) {
                            ForEach(appearanceChoices, id: \.value) { option($0.label, id: $0.id) }
                        }.pickerStyle(.radioGroup).labelsHidden()
                            .background(Marker(name: "appearance.rows"))
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
                    } else {
                        Picker("Text size", selection: scale) {
                            option("Standard", id: sizeChoices[0].id).tag(1.0)
                            option("Large", id: sizeChoices[1].id).tag(1.3)
                            option("Largest", id: sizeChoices[2].id).tag(1.6)
                        }.pickerStyle(.radioGroup).labelsHidden()
                            .background(Marker(name: "text_size.rows"))
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
                for treatment in Treatment.allCases {
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
                    let result: [String: Any] = ["case": key, "kind": owned ? "owned" : "picker", "pointSize": Double(pointSize),
                        "treatment": treatment.rawValue,
                        "offlineGates": ["full-raster picker-original == picker-marked EXACT before marker geometry is trusted",
                            "full-raster owned-original == owned-control EXACT; all owned before/noop checkpoints EXACT",
                            "full-raster owned-structural == retained run30 owned-structural EXACT after passive wrapper markers; mismatch is marker-confound negative",
                            "compare untranslated label/disc pixels and full scene; mismatch is a valid negative, never fit offsets"],
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
        // Eight variants × two themes × six sizes; measurement, NOT visual acceptance.
        XCTAssertEqual(cases.count, 96); XCTAssertEqual(Set(cases).count, 96)
        try JSONSerialization.data(withJSONObject: ["cases": cases, "meaning": "measurement complete, NOT visual acceptance"], options: [.prettyPrinted])
            .write(to: root.appendingPathComponent("cases.json"))
    }
}
#endif
