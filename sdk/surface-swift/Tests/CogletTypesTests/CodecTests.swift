import Foundation
import XCTest
@testable import CogletTypes

// Debug fixtures use JSON, not the wire. Decode integer tokens directly into
// UInt64/Int64 rather than NSNumber/Double (which loses uint64 precision).
private indirect enum FixtureValue: Decodable {
    case value(CBORValue), array([FixtureValue]), map([Swift.String: FixtureValue]), outsideProfile
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .value(.null) }
        else if let b = try? c.decode(Swift.Bool.self) { self = .value(.boolean(b)) }
        else if let n = try? c.decode(Swift.UInt64.self) { self = .value(.unsigned(n)) }
        else if let n = try? c.decode(Swift.Int64.self) { self = .value(.negative(n)) }
        else if let s = try? c.decode(Swift.String.self) { self = .value(.text(s)) }
        else if let a = try? c.decode([FixtureValue].self) { self = .array(a) }
        else if let m = try? c.decode([Swift.String: FixtureValue].self) { self = .map(m) }
        else { _ = try c.decode(Decimal.self); self = .outsideProfile }
    }
    func wireValue() throws -> CBORValue {
        switch self {
        case .value(let v): return v
        case .array(let a): return .array(try a.map { try $0.wireValue() })
        case .map(let m): return .map(try m.map { .init($0.key, try $0.value.wireValue()) })
        case .outsideProfile: throw CBORFailure.invalid("JSON number outside profile")
        }
    }
}
private struct Fixture: Decodable {
    let name: Swift.String
    let type: Swift.String?
    let valid: Swift.Bool?
    let value: FixtureValue?
    let cbor_hex: Swift.String?
}

final class CodecTests: XCTestCase {
    private func hex(_ text: Swift.String) throws -> [Swift.UInt8] {
        let chars = Array(text)
        guard chars.count % 2 == 0 else { throw CBORFailure.invalid("odd hex") }
        return try stride(from: 0, to: chars.count, by: 2).map {
            guard let byte = Swift.UInt8(Swift.String(chars[$0...$0 + 1]), radix: 16) else { throw CBORFailure.invalid("bad hex") }
            return byte
        }
    }

    func testRecursiveFixtures() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let dir = root.appendingPathComponent("spec/fixtures")
        let enumerator = try XCTUnwrap(FileManager.default.enumerator(at: dir, includingPropertiesForKeys: nil))
        let files = enumerator.compactMap { $0 as? URL }.filter { $0.pathExtension == "json" }.sorted { $0.path < $1.path }
        XCTAssertFalse(files.isEmpty)
        var counts = [0, 0, 0]
        for file in files {
            let fixtures = try JSONDecoder().decode([Fixture].self, from: Data(contentsOf: file))
            XCTAssertFalse(fixtures.isEmpty, file.path)
            for f in fixtures {
                let label = file.lastPathComponent + ":" + f.name
                if let valid = f.valid {
                    let rule = try XCTUnwrap(f.type, label)
                    XCTAssertNotNil(GeneratedSchema.rules[rule], "unknown fixture rule: " + rule)
                    let json = try XCTUnwrap(f.value, label)
                    if valid {
                        let golden = try hex(XCTUnwrap(f.cbor_hex, label))
                        let value = try json.wireValue()
                        XCTAssertEqual(try CBORCodec.encode(rule: rule, value: value), golden, label)
                        let decoded = try CBORCodec.decode(rule: rule, bytes: golden)
                        XCTAssertEqual(try CBORCodec.encode(rule: rule, value: decoded), golden, label)
                        XCTAssertEqual(try GeneratedSchema.typedRoundTrip(rule: rule, bytes: golden), golden, label)
                        counts[0] += 1
                    } else {
                        XCTAssertThrowsError(try CBORCodec.encode(rule: rule, value: json.wireValue()), label)
                        // Semantic invalids representable in the profile must also
                        // be rejected at the decoder, not just by encode().
                        if let v = try? json.wireValue(), let raw = try? CBORCodec.encodeProfile(v) {
                            XCTAssertThrowsError(try CBORCodec.decode(rule: rule, bytes: raw), label)
                        }
                        counts[1] += 1
                    }
                } else {
                    guard f.type == nil, f.value == nil, let raw = f.cbor_hex else {
                        throw CBORFailure.invalid("unknown fixture format: " + label)
                    }
                    XCTAssertThrowsError(try CBORCodec.decodeProfile(hex(raw)), label)
                    counts[2] += 1
                }
            }
        }
        XCTAssertGreaterThanOrEqual(counts[0], 33)
        XCTAssertGreaterThanOrEqual(counts[1], 43)
        XCTAssertGreaterThanOrEqual(counts[2], 17)
        print("Swift fixtures: \(counts[0]) golden, \(counts[1]) semantic-invalid, \(counts[2]) wire-invalid")
    }

    func testWireBoundsAndUnicodeIdentity() throws {
        for bad in ["", "1b", "3bffffffffffffffff", "7bffffffffffffffff", "9bffffffffffffffff", "bbffffffffffffffff", "f814", "61ff", "62c080", "63eda080", "a201000200"] {
            XCTAssertThrowsError(try CBORCodec.decodeProfile(hex(bad)), bad)
        }
        let distinct = CBORValue.map([.init("é", .unsigned(1)), .init("e\u{301}", .unsigned(2))])
        let raw = try CBORCodec.encodeProfile(distinct)
        let decoded = try CBORCodec.decodeProfile(raw)
        XCTAssertEqual(try decoded.requireMap().count, 2)
        XCTAssertEqual(try CBORCodec.encodeProfile(decoded), raw)
        XCTAssertThrowsError(try CBORCodec.encodeProfile(.map([.init("é", .null), .init("é", .null)])))
        XCTAssertEqual(try CBORCodec.decodeProfile(hex("1bffffffffffffffff")), .unsigned(.max))
        XCTAssertEqual(try CBORCodec.decodeProfile(hex("3b7fffffffffffffff")), .negative(.min))
        XCTAssertThrowsError(try CBORCodec.encodeProfile(.negative(0)))
        XCTAssertThrowsError(try CBORCodec.decodeProfile(hex("a261620062616100" + "00")))
        XCTAssertEqual(try CBORCodec.encodeProfile(.map([.init("aa", .null), .init("b", .null)])), try hex("a26162f6626161f6"))
    }

    func testDepthLimit() throws {
        var value = CBORValue.null
        for _ in 0..<64 { value = .array([value]) }
        let raw = try CBORCodec.encodeProfile(value)
        XCTAssertEqual(try CBORCodec.encodeProfile(CBORCodec.decodeProfile(raw)), raw)
        XCTAssertEqual(try CBORCodec.encode(rule: "profile-value", value: CBORCodec.decode(rule: "profile-value", bytes: raw)), raw)
        XCTAssertEqual(try ProfileValue.fromCBOR(raw).toCBOR(), raw)
        XCTAssertThrowsError(try CBORCodec.encodeProfile(.array([value])))
        XCTAssertThrowsError(try CBORCodec.decodeProfile([0x81] + raw))
        XCTAssertThrowsError(try CBORCodec.decodeProfile(Array(repeating: 0x81, count: 100_000) + [0xf6]))
    }

    func testTypedBranchesAndValidation() throws {
        let id = Swift.String(repeating: "a", count: 32)
        let accepted = ActionAccept.variant1(.init(id: id, correlation_id: id, accepted: true))
        let raw = try accepted.toCBOR()
        XCTAssertEqual(try ActionAccept.fromCBOR(raw).toCBOR(), raw)
        XCTAssertThrowsError(try ActionAcceptVariant2.fromCBOR(raw))
        XCTAssertThrowsError(try ActionAcceptVariant1(id: id, correlation_id: id, accepted: false).toCBOR())
        XCTAssertThrowsError(try CBORCodec.encode(rule: "node-id", value: .text(Swift.String(repeating: "a", count: 64) + "\n")))
        XCTAssertThrowsError(try CBORCodec.encode(rule: "unknown", value: .null))
    }
}
