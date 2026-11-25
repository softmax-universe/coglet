import XCTest
import Foundation
@testable import SoftmaxAppSurface
@testable import CogletSurface
import CogletTypes

final class ProductCatalogTests: XCTestCase {
    private var directory: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }
    private func bytes(_ name: String) throws -> [UInt8] { Array(try Data(contentsOf: directory.appendingPathComponent(name))) }
    private func pin() throws -> String { String(decoding: try bytes("catalog.v1.sha256"), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
    private func hex(_ value: String) -> [UInt8] {
        stride(from: 0, to: value.count, by: 2).map { offset in
            let start = value.index(value.startIndex, offsetBy: offset)
            return UInt8(value[start..<value.index(start, offsetBy: 2)], radix: 16)!
        }
    }

    func testGeneratedTypesAndAllShapeFixtures() throws {
        let cases = try JSONSerialization.jsonObject(with: Data(try bytes("fixtures.json"))) as! [[String: Any]]
        for item in cases {
            let rule = item["rule"] as! String, name = item["name"] as! String
            let raw = hex(item["cbor_hex"] as! String)
            if item["valid"] as! Bool {
                let roundTrip = try SoftmaxAppSurface.GeneratedSchema.typedRoundTrip(rule: rule, bytes: raw)
                XCTAssertEqual(raw, roundTrip, name)
            } else {
                XCTAssertThrowsError(try SoftmaxAppSurface.GeneratedSchema.typedRoundTrip(rule: rule, bytes: raw), name)
            }
        }
        let empty = AppEmpty()
        XCTAssertEqual(try empty.toCBOR(), [0xa0])
        let focus = AppFocusArgs(window: .variant3("settings"), control: .text("settings.search"))
        XCTAssertEqual(try AppFocusArgs.fromCBOR(focus.toCBOR()).control, .text("settings.search"))
        print("product Swift shape fixtures: \(cases.count)")
    }

    func testRealTrustedLoaderProfilePinAndSelectedSchemas() throws {
        let raw = try bytes("catalog.v1.json"), digest = try pin()
        let catalog = try TrustedExtensionCatalog(json: raw, sha256: digest, contractProfile: .surface02)
        XCTAssertEqual(catalog.operations.count, 13)
        XCTAssertThrowsError(try TrustedExtensionCatalog(json: raw, sha256: digest))
        XCTAssertThrowsError(try TrustedExtensionCatalog(json: raw, sha256: String(repeating: "0", count: 64), contractProfile: .surface02))
        var changed = raw; changed.append(0x20)
        XCTAssertThrowsError(try TrustedExtensionCatalog(json: changed, sha256: digest, contractProfile: .surface02))
        let cases = try JSONSerialization.jsonObject(with: Data(try bytes("fixtures.json"))) as! [[String: Any]]
        let catalogJSON = try JSONSerialization.jsonObject(with: Data(raw)) as! [String: Any]
        let schemas = catalogJSON["schemas"] as! [String: String]
        let operations = catalogJSON["operations"] as! [String: [String: Any]]
        var checks = 0
        for (name, binding) in operations {
            for (key, output) in [("input", false), ("output", true)] {
                guard let uri = binding[key] as? String, let rule = schemas[uri] else { continue }
                for fixture in cases where fixture["rule"] as? String == rule {
                    let raw = hex(fixture["cbor_hex"] as! String)
                    let value: CogletTypes.CBORValue
                    do { value = try CogletTypes.CBORCodec.decodeProfile(raw) }
                    catch { XCTAssertFalse(fixture["valid"] as! Bool); continue }
                    if fixture["valid"] as! Bool { try catalog.check(operation: name, output: output, value: value) }
                    else { XCTAssertThrowsError(try catalog.check(operation: name, output: output, value: value), fixture["name"] as! String) }
                    checks += 1
                }
            }
        }
        XCTAssertThrowsError(try catalog.check(operation: "softmax.app.unknown", output: false, value: .map([])))
        let sibling = CogletTypes.CBORValue.map([.init("window", .text("about"))])
        try catalog.check(operation: "softmax.app.open_window", output: false, value: sibling)
        XCTAssertThrowsError(try catalog.check(operation: "softmax.app.navigate", output: false, value: sibling))
        print("real Swift catalog selected validation checks: \(checks)")
    }
}
