#if os(macOS)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxCore

final class FaceSnapshotTests: XCTestCase, @unchecked Sendable {
    @MainActor
    func testEveryStateInLightAndDark() async throws {
        _ = NSApplication.shared
        let states: [(String, FaceState, FaceExpression)] = [
            ("idle", .idle, .neutral), ("listening", .listening(level: 0.8), .neutral),
            ("thinking", .thinking, .neutral), ("speaking", .speaking(level: 0.7), .neutral),
            ("happy", .idle, .happy), ("concerned", .idle, .concerned)
        ]
        let output = URL(fileURLWithPath: ProcessInfo.processInfo.environment["FACE_SNAPSHOT_OUTPUT"] ?? NSTemporaryDirectory()).appendingPathComponent("FaceSnapshots")
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let goldens = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Snapshots")
        for (name, state, expression) in states {
            for dark in [false, true] {
                let driver = LocalFaceDriver()
                driver.setState(state)
                try driver.setExpression(expression.rawValue, intensity: 1000, durationMS: 60000)
                let view = FaceArtwork(parameters: driver.parameters(reduceMotion: true))
                    .padding(12).frame(width: 280, height: 280)
                    .background(dark ? Color(red: 0.07, green: 0.09, blue: 0.08) : Color(red: 0.96, green: 0.97, blue: 0.96))
                    .environment(\.colorScheme, dark ? .dark : .light)
                let renderer = ImageRenderer(content: view); renderer.scale = 1
                let image = try XCTUnwrap(renderer.cgImage)
                let bitmap = NSBitmapImageRep(cgImage: image)
                let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                let filename = "\(dark ? "dark" : "light")-\(name).png"
                try png.write(to: output.appendingPathComponent(filename))
                let reference = goldens.appendingPathComponent(filename)
                guard let data = try? Data(contentsOf: reference), let expected = NSBitmapImageRep(data: data) else {
                    XCTFail("Missing reviewed native baseline: \(filename); actual saved to \(output.path)"); continue
                }
                XCTAssertEqual(bitmap.pixelsWide, expected.pixelsWide)
                XCTAssertEqual(bitmap.pixelsHigh, expected.pixelsHigh)
                var difference = 0.0
                for y in 0..<280 { for x in 0..<280 {
                    let a = try XCTUnwrap(bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB))
                    let b = try XCTUnwrap(expected.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB))
                    difference += abs(a.redComponent-b.redComponent) + abs(a.greenComponent-b.greenComponent) + abs(a.blueComponent-b.blueComponent)
                } }
                XCTAssertLessThan(difference / (280 * 280 * 3), 0.005, "Visual regression: \(filename)")
            }
        }
    }
}
#endif
