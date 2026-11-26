#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxAppSurface

/// Disposable measurement-only fixture. A pass is NOT visual qualification.
/// No product action, real account, system AX access, or persistent defaults.
@MainActor
final class RadioPresentationMeasurementTests: XCTestCase {
    // Diagnostic interventions ONLY. Replacement-control fidelity is an offline
    // interpretation gate, not implied by successfully drawing these cells.
    private enum Treatment: String, CaseIterable {
        case original, passive, control, ink, label, disc, combined
        var recolor: Bool { self == .ink || self == .combined }
        var moveLabel: Bool { self == .label || self == .combined }
        var centerDisc: Bool { self == .disc || self == .combined }
    }
    private final class PassiveCell: NSButtonCell {}
    private final class ProbeCell: NSButtonCell {
        var treatment: Treatment = .control
        var ink = NSColor.labelColor
        var titleCalls = 0, imageCalls = 0
        var titlePassed = NSRect.zero, titleApplied = NSRect.zero, titleReturned = NSRect.zero
        var imagePassed = NSRect.zero, imageUnsnapped = NSRect.zero, imageApplied = NSRect.zero
        override func drawTitle(_ title: NSAttributedString, withFrame frame: NSRect, in controlView: NSView) -> NSRect {
            titleCalls += 1; titlePassed = frame
            var adjusted = frame
            if treatment.moveLabel {
                var backing = controlView.convertToBacking(adjusted)
                backing.origin.x -= 1 // ONE actual raster pixel, not one point.
                adjusted = controlView.convertFromBacking(backing)
            }
            titleApplied = adjusted
            if treatment.recolor {
                let text = NSMutableAttributedString(attributedString: title)
                text.addAttribute(.foregroundColor, value: ink, range: NSRange(location: 0, length: text.length))
                titleReturned = super.drawTitle(text, withFrame: adjusted, in: controlView)
            } else {
                titleReturned = super.drawTitle(title, withFrame: adjusted, in: controlView)
            }
            return titleReturned
        }
        override func drawImage(_ image: NSImage, withFrame frame: NSRect, in controlView: NSView) {
            imageCalls += 1; imagePassed = frame
            var adjusted = frame
            if treatment.centerDisc {
                adjusted.origin.y = controlView.bounds.midY - frame.height / 2
                imageUnsnapped = adjusted
                var backing = controlView.convertToBacking(adjusted)
                // Fixed hypothesis: nearest pixel; half ties away from zero.
                backing.origin.y = backing.origin.y.rounded(.toNearestOrAwayFromZero)
                adjusted = controlView.convertFromBacking(backing)
            } else { imageUnsnapped = adjusted }
            imageApplied = adjusted
            super.drawImage(image, withFrame: adjusted, in: controlView)
        }
    }
    private struct Marker: NSViewRepresentable {
        let name: String
        func makeNSView(context: Context) -> NSView {
            let v = NSView(); v.identifier = NSUserInterfaceItemIdentifier(name); return v
        }
        func updateNSView(_ v: NSView, context: Context) {}
    }
    private struct Fixture: View {
        let owned: Bool, dark: Bool, pointSize: CGFloat
        let registry: ControlRegistry
        @State private var appearance = "System"
        @State private var scale = 1.0
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
                            window: "settings", route: "general", choices: [
                                .init(value: "System", label: "System", id: "settings.general.appearance.system"),
                                .init(value: "Light", label: "Light", id: "settings.general.appearance.light"),
                                .init(value: "Dark", label: "Dark", id: "settings.general.appearance.dark")],
                            selection: $appearance, fontSize: pointSize, failure: { XCTFail("\($0)") })
                            .background(Marker(name: "appearance.rows"))
                    } else {
                        Picker("Appearance", selection: $appearance) {
                            ForEach(["System", "Light", "Dark"], id: \.self) { Text($0) }
                        }.pickerStyle(.radioGroup).labelsHidden()
                            .background(Marker(name: "appearance.rows"))
                    }
                }.background(Marker(name: "appearance.group"))
                VStack(alignment: .leading, spacing: 4) {
                    Text("Text size").font(.system(size: pointSize)).accessibilityHidden(true)
                    if owned {
                        CogletRadioGroup("Text size", id: "settings.general.text_size", registry: registry,
                            window: "settings", route: "general", choices: [
                                .init(value: "standard", label: "Standard", id: "settings.general.text_size.standard"),
                                .init(value: "large", label: "Large", id: "settings.general.text_size.large"),
                                .init(value: "largest", label: "Largest", id: "settings.general.text_size.largest")],
                            selection: Binding(get: { scale == 1 ? "standard" : scale == 1.3 ? "large" : "largest" },
                                set: { scale = $0 == "standard" ? 1 : $0 == "large" ? 1.3 : 1.6 }),
                            fontSize: pointSize, failure: { XCTFail("\($0)") })
                            .background(Marker(name: "text_size.rows"))
                    } else {
                        Picker("Text size", selection: $scale) {
                            Text("Standard").tag(1.0); Text("Large").tag(1.3); Text("Largest").tag(1.6)
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
            "axRole": v.accessibilityRole()?.rawValue ?? "notExposed", "axID": v.accessibilityIdentifier() ?? "notExposed"]
        if let c = v as? NSControl {
            d["font"] = font(c.font); d["enabled"] = c.isEnabled; d["controlSize"] = c.controlSize.rawValue
        }
        if let b = v as? NSButton {
            d["title"] = b.title; d["state"] = b.state.rawValue
            d["imagePosition"] = b.imagePosition.rawValue; d["imageScaling"] = b.imageScaling.rawValue
            d["contentTint"] = color(b.contentTintColor)
            var runs: [[String: Any]] = []
            b.attributedTitle.enumerateAttributes(in: NSRange(location: 0, length: b.attributedTitle.length)) { attrs, range, _ in
                runs.append(["range": [range.location, range.length], "attributes": attrs.map { key, val in
                    ["key": key.rawValue, "description": String(describing: val)]
                }])
            }
            d["titleAttributeRuns"] = runs
            if let probe = b.cell as? ProbeCell {
                d["probe"] = ["treatment": probe.treatment.rawValue, "titleCalls": probe.titleCalls, "imageCalls": probe.imageCalls,
                    "titlePassed": rect(probe.titlePassed), "titleApplied": rect(probe.titleApplied), "titleReturned": rect(probe.titleReturned),
                    "imagePassed": rect(probe.imagePassed), "imageUnsnapped": rect(probe.imageUnsnapped), "imageApplied": rect(probe.imageApplied),
                    "backingBounds": rect(b.convertToBacking(b.bounds)), "highlighted": probe.isHighlighted]
            }
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
        d["children"] = v.subviews.enumerated().map { metadata($0.element, host: host, path: path + "." + String($0.offset)) }
        return d
    }
    private func views(_ v: NSView) -> [NSView] { [v] + v.subviews.flatMap { views($0) } }
    private func capture(_ host: NSView) throws -> (Data, NSBitmapImageRep) {
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        return (try XCTUnwrap(bitmap.representation(using: .png, properties: [:])), bitmap)
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
            for pointSize in [CGFloat(14), CGFloat(18.2), CGFloat(22.4)] {
                let variants: [(Bool, Treatment)] = [(false, .original)] + Treatment.allCases.map { (true, $0) }
                for (owned, treatment) in variants {
                    let key = "\(dark ? "dark" : "light")-\(pointSize)-\(owned ? "owned-" + treatment.rawValue : "picker")"
                    let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
                    // MainActor serial fixture only: let native initialization own
                    // every cell property. NEVER copy or replace an existing cell.
                    let savedClass = NSButton.cellClass
                    if owned && treatment != .original {
                        NSButton.cellClass = treatment == .passive ? PassiveCell.self : ProbeCell.self
                    }
                    let expectedClass = NSButton.cellClass
                    defer {
                        NSButton.cellClass = savedClass
                        XCTAssertEqual(NSButton.cellClass.map { ObjectIdentifier($0) }, savedClass.map { ObjectIdentifier($0) })
                    }
                    let host = NSHostingView(rootView: Fixture(owned: owned, dark: dark, pointSize: pointSize, registry: registry))
                    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 440),
                        styleMask: [.borderless], backing: .buffered, defer: false)
                    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                    window.isReleasedWhenClosed = false
                    defer { window.endEditing(for: nil); window.makeFirstResponder(nil); window.contentView = nil; window.close() }
                    window.contentView = host; host.frame = NSRect(x: 0, y: 0, width: 540, height: 440)
                    window.orderBack(nil); window.makeFirstResponder(nil)
                    // Stabilize the SAME native graph before changing its cells.
                    RunLoop.current.run(until: Date().addingTimeInterval(0.25))
                    host.layoutSubtreeIfNeeded(); window.display()
                    let nativeButtons = views(host).compactMap { $0 as? NSButton }
                    let initialButtons = nativeButtons.map(ObjectIdentifier.init)
                    let initialTargets = nativeButtons.map { $0.target.map(ObjectIdentifier.init) }
                    let initialActions = nativeButtons.map { $0.action }
                    let initialStates = nativeButtons.map { $0.state }
                    let initialFrames = nativeButtons.map { $0.frame }
                    let initialIDs: [String?] = nativeButtons.map { $0.accessibilityIdentifier() }
                    let initialCells = nativeButtons.map { $0.cell.map(ObjectIdentifier.init) }
                    if owned {
                        XCTAssertEqual(nativeButtons.count, 6)
                        let wanted = expectedClass ?? NSButtonCell.self
                        for button in nativeButtons {
                            let cell = try XCTUnwrap(button.cell)
                            XCTAssertEqual(ObjectIdentifier(type(of: cell)), ObjectIdentifier(wanted), "Factory was not inherited: " + key)
                        }
                    }
                    let initialInventory = registry.registrationInventory()
                    let initialMounts = try initialIDs.compactMap { id -> ControlRegistry.Mount? in
                        guard owned, let id else { return nil }
                        return try registry.captureDirectory().prepareFocus(.map([
                            .init("window", .text("settings")), .init("control", .text(id))]))
                    }
                    if owned && treatment != .original {
                        let before = try capture(host)
                        try before.0.write(to: root.appendingPathComponent(key + "-before.png"))
                        try JSONSerialization.data(withJSONObject: metadata(host, host: host, path: "root"), options: [.prettyPrinted, .sortedKeys])
                            .write(to: root.appendingPathComponent(key + "-before.json"))
                        if treatment != .passive {
                            for button in nativeButtons {
                                let probe = try XCTUnwrap(button.cell as? ProbeCell)
                                probe.treatment = treatment
                                probe.ink = NSColor(srgbRed: dark ? 0.925490196 : 0.090196078,
                                    green: dark ? 0.960784314 : 0.137254902, blue: dark ? 0.937254902 : 0.113725490, alpha: 1)
                                button.needsDisplay = true
                            }
                        }
                    }
                    let start = Date(); var previous: Data?; var count = 0
                    var stable: (Data, NSBitmapImageRep)?
                    while Date().timeIntervalSince(start) < 2 {
                        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
                        host.layoutSubtreeIfNeeded(); window.display()
                        let current = try capture(host)
                        count = current.0 == previous ? count + 1 : 1; previous = current.0
                        if count >= 3 && Date().timeIntervalSince(start) >= 0.2 { stable = current; break }
                    }
                    _ = try XCTUnwrap(stable, "Unstable fixture: \(key)")
                    for button in nativeButtons {
                        if let probe = button.cell as? ProbeCell { probe.titleCalls = 0; probe.imageCalls = 0 }
                        button.needsDisplay = true
                    }
                    host.needsDisplay = true
                    let (png, bitmap) = try capture(host)
                    XCTAssertEqual(png, stable?.0, "Forced capture changed stable pixels: " + key)
                    // Retain the pixels/metadata even if an invariant or hook fails.
                    try png.write(to: root.appendingPathComponent(key + ".png"))
                    let all = views(host)
                    if owned {
                        let buttons = all.compactMap { $0 as? NSButton }
                        XCTAssertEqual(buttons.count, 6)
                        XCTAssertEqual(Set(buttons.map(\.title)), Set(["System", "Light", "Dark", "Standard", "Large", "Largest"]))
                        XCTAssertEqual(buttons.filter { $0.state == .on }.count, 2)
                        XCTAssertTrue(buttons.allSatisfy { $0.font?.pointSize == pointSize })
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
                        XCTAssertEqual(views(host).compactMap { $0 as? NSButton }.map(ObjectIdentifier.init), initialButtons)
                        XCTAssertEqual(nativeButtons.map { $0.target.map(ObjectIdentifier.init) }, initialTargets)
                        XCTAssertEqual(nativeButtons.map { $0.action }, initialActions)
                        XCTAssertEqual(nativeButtons.map { $0.state }, initialStates)
                        XCTAssertEqual(nativeButtons.map { $0.frame }, initialFrames)
                        XCTAssertEqual(nativeButtons.map { $0.accessibilityIdentifier() }, initialIDs)
                        XCTAssertEqual(nativeButtons.map { $0.cell.map(ObjectIdentifier.init) }, initialCells)
                        XCTAssertEqual(registry.registrationInventory(), initialInventory)
                        let finalMounts = try initialIDs.compactMap { id -> ControlRegistry.Mount? in
                            guard let id else { return nil }
                            return try registry.captureDirectory().prepareFocus(.map([
                                .init("window", .text("settings")), .init("control", .text(id))]))
                        }
                        XCTAssertEqual(finalMounts, initialMounts)
                        if treatment != .original && treatment != .passive {
                            for button in nativeButtons {
                                let probe = try XCTUnwrap(button.cell as? ProbeCell)
                                XCTAssertGreaterThan(probe.titleCalls, 0, key + ": uncalled title hook " + button.title)
                                XCTAssertGreaterThan(probe.imageCalls, 0, key + ": uncalled image hook " + button.title)
                                XCTAssertEqual(probe.imagePassed.size, NSSize(width: 16, height: 16))
                            }
                        }
                    }
                }
            }
        }
        XCTAssertEqual(cases.count, 48); XCTAssertEqual(Set(cases).count, 48)
        try JSONSerialization.data(withJSONObject: ["cases": cases, "meaning": "measurement complete, NOT visual acceptance"], options: [.prettyPrinted])
            .write(to: root.appendingPathComponent("cases.json"))
    }
}
#endif
