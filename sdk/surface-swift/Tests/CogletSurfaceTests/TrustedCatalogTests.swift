import Foundation
import XCTest
import CogletTypes
@testable import CogletSurface

final class TrustedCatalogTests: XCTestCase {
    private let empty: [String: Any] = ["op": "record", "fields": []]
    private func literal(_ value: Any) -> [String: Any] { ["op": "literal", "value": value] }
    private func fixture(_ operation: String = "widget.set") -> [String: Any] {
        ["version": 1, "namespace": "widget",
         "rules": ["empty": empty, "input": ["op": "record", "fields": [
            ["name": "value", "optional": false, "type": ["op": "range", "min": 1, "max": 3]]]]],
         "schemas": ["https://example.org/widget/input": "input", "https://example.org/widget/output": "empty"],
         "entries": ["widget.control": ["widget.device"]],
         "operations": [operation: ["entry": "widget.control", "direction": "action",
            "input": "https://example.org/widget/input", "output": "https://example.org/widget/output",
            "resources": ["widget.device"], "single_owner": true]]]
    }
    private func bytes(_ object: [String: Any]) throws -> [UInt8] {
        Array(try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]))
    }
    private func load(_ object: [String: Any], profile: SurfaceContractProfile = .surface01) throws -> TrustedExtensionCatalog {
        let raw = try bytes(object)
        return try TrustedExtensionCatalog(json: raw, sha256: catalogDigest(raw), contractProfile: profile)
    }
    private func loadRaw(_ raw: [UInt8]) throws -> TrustedExtensionCatalog {
        try TrustedExtensionCatalog(json: raw, sha256: catalogDigest(raw))
    }
    private func withRule(_ rule: [String: Any]) -> [String: Any] {
        var f = fixture(); var rules = f["rules"] as! [String: Any]; rules["input"] = rule; f["rules"] = rules; return f
    }
    private func record(_ entries: [(String, CBORValue)]) -> CBORValue { .map(entries.map { CBORMapEntry($0.0, $0.1) }) }

    func testExactByteRuleAndSchemaReferences() throws {
        let composed = "é", decomposed = "e\u{301}"
        var f = fixture()
        f["rules"] = [composed: literal("selected"), "input": ["op": "ref", "name": decomposed], "empty": empty]
        XCTAssertThrowsError(try load(f))
        f["rules"] = [composed: literal("selected"), "input": ["op": "ref", "name": composed], "empty": empty]
        try load(f).check(operation: "widget.set", output: false, value: .text("selected"))
        f["schemas"] = ["https://example.org/widget/input": decomposed, "https://example.org/widget/output": "empty"]
        XCTAssertThrowsError(try load(f))
        var uriAlias = fixture()
        uriAlias["schemas"] = ["https://example.org/é": "input", "https://example.org/widget/output": "empty"]
        var ops = uriAlias["operations"] as! [String: [String: Any]]
        ops["widget.set"]!["input"] = "https://example.org/e\u{301}"
        uriAlias["operations"] = ops
        XCTAssertThrowsError(try load(uriAlias))
    }

    func testSHA256KnownVectors() {
        XCTAssertEqual(catalogDigest([]), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        XCTAssertEqual(catalogDigest(Array("abc".utf8)), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    }

    func testPinnedImmutableCatalogAndSelectedSchemas() throws {
        var raw = try bytes(fixture())
        let pin = catalogDigest(raw)
        let catalog = try TrustedExtensionCatalog(json: raw, sha256: pin)
        raw.removeAll() // the catalog owns parsed immutable values, not caller storage
        XCTAssertEqual(catalog.pin, pin)
        XCTAssertEqual(catalog.contractProfile, .surface01)
        XCTAssertEqual(catalog.operations.map(\.name), ["widget.set"])
        XCTAssertEqual(catalog.operations[0].inputRule, "input")
        XCTAssertEqual(catalog.operations[0].outputRule, "empty")
        XCTAssertEqual(catalog.operations[0].resources, ["widget.device"])
        XCTAssertEqual(catalog.schemaURI(operation: "widget.set"), "https://example.org/widget/input")
        for n: UInt64 in [1, 3] { try catalog.check(operation: "widget.set", output: false, value: record([("value", .unsigned(n))])) }
        try catalog.check(operation: "widget.set", output: true, value: .map([]))
        for n: UInt64 in [0, 4] { XCTAssertThrowsError(try catalog.check(operation: "widget.set", output: false, value: record([("value", .unsigned(n))]))) }
        XCTAssertThrowsError(try catalog.check(operation: "widget.set", output: false, value: .map([])))
        XCTAssertThrowsError(try catalog.check(operation: "widget.set", output: false, value: record([("value", .unsigned(1)), ("extra", .null)])))
        XCTAssertThrowsError(try catalog.check(operation: "widget.set", output: true, value: record([("value", .unsigned(1))])))
        XCTAssertThrowsError(try catalog.check(operation: "widget.unknown", output: false, value: .map([])))
        XCTAssertThrowsError(try catalog.check(operation: "widget.set", output: false, value: record([("value", .unsigned(1)), ("value", .unsigned(1))])))
    }

    func testWrongMissingMalformedAndExactBytePins() throws {
        let raw = try bytes(fixture()), pin = catalogDigest(raw)
        for bad in ["", String(repeating: "0", count: 64), pin.uppercased(), pin + "0", " " + pin] {
            XCTAssertThrowsError(try TrustedExtensionCatalog(json: raw, sha256: bad))
        }
        XCTAssertThrowsError(try TrustedExtensionCatalog(json: raw + [32], sha256: pin))
        XCTAssertNoThrow(try loadRaw(raw + [32]))
    }

    func testStrictJSONAndArtifactClosure() throws {
        let valid = String(decoding: try bytes(fixture()), as: UTF8.self)
        for raw in [valid + "true", valid + "{}", "{\"version\":1," + valid.dropFirst(),
                    "{\"vers\\u0069on\":1," + valid.dropFirst(),
                    valid.replacingOccurrences(of: "\"value\":", with: "\"value\":0,\"value\":"),
                    valid.replacingOccurrences(of: "\"version\":1", with: "\"version\":1.0"),
                    valid.replacingOccurrences(of: "\"version\":1", with: "\"version\":01"),
                    valid.replacingOccurrences(of: "\"version\":1", with: "\"version\":1e0")] {
            // Duplicate literal key variant is separately covered below; only
            // apply replacements that actually changed the source fixture.
            if raw != valid { XCTAssertThrowsError(try loadRaw(Array(raw.utf8)), raw) }
        }
        let duplicateNested = valid.replacingOccurrences(of: "\"single_owner\":true", with: "\"single_owner\":true,\"single_owner\":true")
        XCTAssertThrowsError(try loadRaw(Array(duplicateNested.utf8)))
        XCTAssertThrowsError(try loadRaw([0xff] + Array(valid.utf8)))
        XCTAssertThrowsError(try loadRaw(Array(valid.replacingOccurrences(of: "widget.set", with: "\\uD800").utf8)))
        var unknown = fixture(); unknown["contract_profile"] = "surface-0.2"
        XCTAssertThrowsError(try load(unknown))
        for version: Any in [0, 2, "1", true] {
            var f = fixture(); f["version"] = version; XCTAssertThrowsError(try load(f))
        }
    }

    func testByteLimitInclusiveBoundary() throws {
        let raw = try bytes(fixture())
        let atLimit = raw + [UInt8](repeating: 32, count: 1_048_576 - raw.count)
        XCTAssertNoThrow(try loadRaw(atLimit))
        XCTAssertThrowsError(try loadRaw(atLimit + [32]))
    }

    func testExplicitProfileAndNarrowNames() throws {
        let original = String(decoding: try bytes(fixture()), as: UTF8.self)
        func app(_ operation: String) throws -> [UInt8] {
            Array(original.replacingOccurrences(of: "widget.set", with: operation)
                .replacingOccurrences(of: "widget", with: "softmax").utf8)
        }
        let raw = try app("softmax.app.set")
        XCTAssertThrowsError(try TrustedExtensionCatalog(json: raw, sha256: catalogDigest(raw)))
        let selected = try TrustedExtensionCatalog(json: raw, sha256: catalogDigest(raw), contractProfile: .surface02)
        XCTAssertEqual(selected.contractProfile, .surface02)
        XCTAssertEqual(selected.operations.first?.name, "softmax.app.set")
        XCTAssertNoThrow(try load(fixture(), profile: .surface02))
        for bad in ["softmax.app.set.extra", "softmax.application.set", "softmax.app.", "softmax.app.Set", "other.app.set"] {
            let bytes = try app(bad)
            XCTAssertThrowsError(try TrustedExtensionCatalog(json: bytes, sha256: catalogDigest(bytes), contractProfile: .surface02), bad)
        }
    }

    func testBindingsNamespacesResourcesAndSchemaOverridesFailClosed() throws {
        for namespace in ["", "speech", "expression", "runtime", "test", "sim", "sensor", "motor", "other", "llm", "action", "hub", "widget.bad"] {
            var f = fixture(); f["namespace"] = namespace; XCTAssertThrowsError(try load(f), namespace)
        }
        for uri in ["http://example.org/schema", "https://coglet.dev/spec/messages/fake", "https://example.org/bad uri"] {
            var f = fixture(); f["schemas"] = [uri: "input"]; XCTAssertThrowsError(try load(f), uri)
        }
        var unknownRule = fixture(); unknownRule["schemas"] = ["https://example.org/widget/input": "missing"]
        XCTAssertThrowsError(try load(unknownRule))
        for (key, value): (String, Any) in [("entry", "widget.missing"), ("direction", "unknown"), ("single_owner", false),
            ("resources", ["widget.missing"]), ("resources", ["widget.device", "widget.device"]),
            ("input", "https://example.org/missing"), ("output", NSNull()), ("unknown", true)] {
            var f = fixture(); var ops = f["operations"] as! [String: [String: Any]]
            ops["widget.set"]![key] = value; f["operations"] = ops; XCTAssertThrowsError(try load(f), key)
        }
        var f = fixture(); f["entries"] = ["widget.control": ["widget.device", "widget.device"]]
        XCTAssertThrowsError(try load(f))
        f = fixture(); f["operations"] = [:]; XCTAssertThrowsError(try load(f))
        f = fixture(); f["rules"] = [:]; XCTAssertThrowsError(try load(f))
    }

    func testAllClosedIRNodesAndIntegerExtremes() throws {
        let nullableText: [String: Any] = ["op": "union", "items": [["op": "primitive", "name": "null"],
            ["op": "intersection", "items": [["op": "primitive", "name": "tstr"], ["op": "size", "min": 1, "max": 4]]]]]
        let c = try load(withRule(nullableText))
        for value: CBORValue in [.null, .text("éé"), .text("a")] { try c.check(operation: "widget.set", output: false, value: value) }
        for value: CBORValue in [.text(""), .text("ééa"), .boolean(true)] { XCTAssertThrowsError(try c.check(operation: "widget.set", output: false, value: value)) }
        let integers = try load(withRule(["op": "range", "min": Int64.min, "max": UInt64.max]))
        for value: CBORValue in [.negative(Int64.min), .unsigned(UInt64.max)] { try integers.check(operation: "widget.set", output: false, value: value) }
        XCTAssertThrowsError(try integers.check(operation: "widget.set", output: false, value: .boolean(true)))
        let array = try load(withRule(["op": "array", "min": 1, "max": 2, "item": ["op": "primitive", "name": "bool"]]))
        try array.check(operation: "widget.set", output: false, value: .array([.boolean(true), .boolean(false)]))
        for value: CBORValue in [.array([]), .array([.boolean(true), .boolean(true), .boolean(true)]), .array([.null])] {
            XCTAssertThrowsError(try array.check(operation: "widget.set", output: false, value: value))
        }
        let mapLiteral = try load(withRule(literal(["a": [1, 2], "b": true])))
        try mapLiteral.check(operation: "widget.set", output: false, value: record([("b", .boolean(true)), ("a", .array([.unsigned(1), .unsigned(2)]))]))
    }

    func testMalformedUnknownOpenAndUnboundedIRRejectedAtLoad() throws {
        let invalid: [[String: Any]] = [
            ["op": "any"], ["op": "primitive", "name": "uint"], ["op": "primitive", "name": "tstr", "extra": true],
            ["op": "ref", "name": "missing"], ["op": "ref", "name": "input"],
            ["op": "map", "min": 0, "max": 1, "key": empty, "value": empty],
            ["op": "record", "fields": [], "rest": [:]], ["op": "union", "items": []], ["op": "intersection", "items": []],
            ["op": "record", "fields": [["name": "a", "optional": false, "type": empty], ["name": "a", "optional": true, "type": empty]]],
            ["op": "record", "fields": [["name": "a", "optional": 0, "type": empty]]],
            ["op": "array", "min": 0, "item": empty], ["op": "array", "min": 0, "max": NSNull(), "item": empty],
            ["op": "array", "min": 0, "max": 262_145, "item": empty], ["op": "array", "min": 2, "max": 1, "item": empty],
            ["op": "range", "min": 2, "max": 1], ["op": "range", "min": 0.5, "max": 2],
            ["op": "size", "min": -1, "max": 1], ["op": "literal", "value": 1.5], ["op": "literal"]]
        for rule in invalid { XCTAssertThrowsError(try load(withRule(rule)), "\(rule)") }
        XCTAssertNoThrow(try load(withRule(["op": "array", "min": 0, "max": 262_144, "item": empty])))
        var unused = fixture(); var rules = unused["rules"] as! [String: Any]; rules["unused"] = ["op": "unknown"]; unused["rules"] = rules
        XCTAssertThrowsError(try load(unused))
        rules = ["input": ["op": "ref", "name": "other"], "other": ["op": "ref", "name": "input"], "empty": empty]
        unused = fixture(); unused["rules"] = rules; XCTAssertThrowsError(try load(unused))
    }

    func testReferenceDepth64Accepted65Rejected() throws {
        for depth in [64, 65] {
            var f = fixture(); var rules: [String: Any] = ["empty": empty, "r\(depth)": empty]
            for i in 0..<depth { rules["r\(i)"] = ["op": "ref", "name": "r\(i + 1)"] }
            // input itself is the first link, not an additional transparent hop.
            rules["input"] = rules.removeValue(forKey: "r0")
            f["rules"] = rules
            if depth == 64 { XCTAssertNoThrow(try load(f)) } else { XCTAssertThrowsError(try load(f)) }
        }
    }

    func testPortableRegexScalarBoundsAndDialectRejection() throws {
        let bounded = try load(withRule(["op": "regexp", "pattern": "^[\\s\\S]{1,2}$"]))
        for text in ["\n", "é", "😀😀", "e\u{301}"] { try bounded.check(operation: "widget.set", output: false, value: .text(text)) }
        for text in ["", "abc"] { XCTAssertThrowsError(try bounded.check(operation: "widget.set", output: false, value: .text(text))) }
        let ascii = try load(withRule(["op": "regexp", "pattern": "^[a-z][a-z0-9_]{0,3}$"]))
        try ascii.check(operation: "widget.set", output: false, value: .text("a_2"))
        for text in ["A", "a\n", "é", "a1234"] { XCTAssertThrowsError(try ascii.check(operation: "widget.set", output: false, value: .text(text))) }
        for pattern in ["\\w+", "\\d+", "(?=a)a", "(a)\\1", "\\p{L}", ".*", "[z-a]", "[a-z]{3,2}", "(?:a|b)"] {
            XCTAssertThrowsError(try load(withRule(["op": "regexp", "pattern": pattern])), pattern)
        }
    }

    func testSelectedReadSiblingValidationAndBindings() throws {
        var f = fixture(); var rules = f["rules"] as! [String: Any]
        rules["event"] = ["op": "record", "fields": [
            ["name": "operation", "optional": false, "type": literal("widget.read")],
            ["name": "args", "optional": false, "type": empty],
            ["name": "value", "optional": false, "type": ["op": "primitive", "name": "tstr"]]]]
        rules["result"] = literal("selected")
        f["rules"] = rules
        var schemas = f["schemas"] as! [String: String]
        schemas["https://example.org/widget/event"] = "event"; schemas["https://example.org/widget/result"] = "result"; f["schemas"] = schemas
        let selection: [String: Any] = ["operation_field": "operation", "args_field": "args", "value_field": "value", "reads": ["widget.read"]]
        f["operations"] = [
            "widget.read": ["entry": "widget.control", "direction": "read", "input": "https://example.org/widget/output", "output": "https://example.org/widget/result", "resources": [], "single_owner": true],
            "widget.observation": ["entry": "widget.control", "direction": "event", "input": "https://example.org/widget/event", "resources": [], "single_owner": true, "selection": selection]]
        let c = try load(f)
        XCTAssertEqual(c.schemaURI(operation: "widget.read"), "https://example.org/widget/result")
        let good = record([("operation", .text("widget.read")), ("args", .map([])), ("value", .text("selected"))])
        try c.check(operation: "widget.observation", output: false, value: good)
        XCTAssertEqual(try c.selectedRead(operation: "widget.observation", value: good), "widget.read")
        // The event's broad tstr sibling passes, but the selected read's exact
        // output literal must still reject it (no root-union validation).
        let bad = record([("operation", .text("widget.read")), ("args", .map([])), ("value", .text("sibling"))])
        XCTAssertThrowsError(try c.check(operation: "widget.observation", output: false, value: bad))
        XCTAssertThrowsError(try c.check(operation: "widget.observation", output: true, value: .map([])))
        var ops = f["operations"] as! [String: [String: Any]]
        var badSelection = selection; badSelection["reads"] = ["widget.missing"]
        ops["widget.observation"]!["selection"] = badSelection; f["operations"] = ops
        XCTAssertThrowsError(try load(f))
        badSelection = selection; badSelection["args_field"] = "value"
        ops["widget.observation"]!["selection"] = badSelection; f["operations"] = ops
        XCTAssertThrowsError(try load(f))
    }
}
