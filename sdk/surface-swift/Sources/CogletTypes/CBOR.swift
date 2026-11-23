import Foundation

public enum CBORFailure: Swift.Error, Equatable {
    case invalid(Swift.String)
}

/// Maps use entries, not Dictionary<String, ...>: Swift String equality performs
/// Unicode normalization, while the protocol compares exact UTF-8 key bytes.
public struct CBORMapEntry: Equatable, Sendable {
    public var key: Swift.String
    public var value: CBORValue
    public init(_ key: Swift.String, _ value: CBORValue) {
        self.key = key
        self.value = value
    }
    public static func == (lhs: Self, rhs: Self) -> Swift.Bool {
        lhs.key.utf8.elementsEqual(rhs.key.utf8) && lhs.value == rhs.value
    }
}

public indirect enum CBORValue: Sendable, Equatable {
    case unsigned(Swift.UInt64)
    /// Must be strictly negative; checked on encoding.
    case negative(Swift.Int64)
    case text(Swift.String)
    case boolean(Swift.Bool)
    case null
    case array([CBORValue])
    case map([CBORMapEntry])

    public static func == (lhs: Self, rhs: Self) -> Swift.Bool {
        switch (lhs, rhs) {
        case let (.unsigned(a), .unsigned(b)): return a == b
        case let (.negative(a), .negative(b)): return a == b
        case let (.text(a), .text(b)): return a.utf8.elementsEqual(b.utf8)
        case let (.boolean(a), .boolean(b)): return a == b
        case (.null, .null): return true
        case let (.array(a), .array(b)): return a == b
        case let (.map(a), .map(b)): return a == b
        default: return false
        }
    }
    internal func requireMap() throws -> [CBORMapEntry] {
        guard case .map(let entries) = self else { throw CBORFailure.invalid("expected map") }
        return entries
    }
}

public protocol CBORConvertible: Sendable {
    init(cborValue: CBORValue, depth: Swift.Int) throws
    func cborValue(depth: Swift.Int) throws -> CBORValue
}

public protocol CBORMessage: CBORConvertible {
    static var schemaRule: Swift.String { get }
}

extension CBORMessage {
    public func toCBOR() throws -> [Swift.UInt8] {
        try CBORCodec.encode(rule: Self.schemaRule, value: cborValue(depth: 0))
    }
    public static func fromCBOR(_ bytes: [Swift.UInt8]) throws -> Self {
        try Self.init(cborValue: CBORCodec.decode(rule: Self.schemaRule, bytes: bytes), depth: 0)
    }
}

extension CBORValue: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        _ = try CBORCodec.encodeProfile(cborValue)
        self = cborValue
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        return self
    }
}

extension Swift.String: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        guard case .text(let s) = cborValue else { throw CBORFailure.invalid("expected text") }
        self = s
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        return .text(self)
    }
}
extension Swift.Bool: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        guard case .boolean(let b) = cborValue else { throw CBORFailure.invalid("expected bool") }
        self = b
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        return .boolean(self)
    }
}
extension Swift.UInt64: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        guard case .unsigned(let n) = cborValue else { throw CBORFailure.invalid("expected uint64") }
        self = n
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        return .unsigned(self)
    }
}
extension Swift.Int64: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        switch cborValue {
        case .negative(let n) where n < 0: self = n
        case .unsigned(let n) where n <= Swift.UInt64(Swift.Int64.max): self = Swift.Int64(n)
        default: throw CBORFailure.invalid("expected int64")
        }
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        return self < 0 ? .negative(self) : .unsigned(Swift.UInt64(self))
    }
}
// Entries are convertible only as part of a map; they are never wire arrays.
extension CBORMapEntry: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        throw CBORFailure.invalid("map entry outside map")
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        throw CBORFailure.invalid("map entry outside map")
    }
}
extension Swift.Array: CBORConvertible where Element: CBORConvertible {
    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {
        try CBORCodec.checkDepth(depth)
        if Element.self == CBORMapEntry.self {
            let entries = try cborValue.requireMap()
            // The checked element type makes this cast exact, never a wire cast.
            guard let result = entries as? Self else { throw CBORFailure.invalid("map type") }
            self = result
        } else {
            guard case .array(let values) = cborValue else { throw CBORFailure.invalid("expected array") }
            self = try values.map { try Element.init(cborValue: $0, depth: depth + 1) }
        }
    }
    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {
        try CBORCodec.checkDepth(depth)
        if let entries = self as? [CBORMapEntry], Element.self == CBORMapEntry.self {
            return .map(entries)
        }
        return .array(try map { try $0.cborValue(depth: depth + 1) })
    }
}

internal struct SchemaField: Sendable {
    let name: Swift.String
    let optional: Swift.Bool
    let node: SchemaNode
}
internal struct SchemaTupleItem: Sendable {
    let min: Swift.UInt64
    let max: Swift.UInt64?
    let node: SchemaNode
}
internal indirect enum SchemaNode: Sendable {
    case reference(Swift.String), union([SchemaNode]), literal(CBORValue)
    case intersection([SchemaNode]), defaultValue(SchemaNode), tuple([SchemaTupleItem])
    case text, boolean
    case range(CBORValue, CBORValue), regexp(Swift.String), size(Swift.UInt64, Swift.UInt64)
    case boundedArray(Swift.UInt64, Swift.UInt64?, SchemaNode)
    case boundedMap(Swift.UInt64, Swift.UInt64?, SchemaNode, SchemaNode)
    case record([SchemaField])
    case openRecord([SchemaField], Swift.UInt64, Swift.UInt64?, SchemaNode, SchemaNode)
}

public enum CBORCodec {
    public static let maxDepth = 64
    public static func checkDepth(_ depth: Swift.Int) throws {
        guard depth >= 0 && depth <= maxDepth else { throw CBORFailure.invalid("nesting limit") }
    }
    internal static func field(_ entries: [CBORMapEntry], _ key: Swift.String) -> CBORValue? {
        entries.first { $0.key.utf8.elementsEqual(key.utf8) }?.value
    }
    internal static func required(_ value: CBORValue?) throws -> CBORValue {
        guard let value else { throw CBORFailure.invalid("missing field") }
        return value
    }
    public static func encode(rule: Swift.String, value: CBORValue) throws -> [Swift.UInt8] {
        let raw = try encodeProfile(value)
        try validate(rule, value)
        return raw
    }
    public static func decode(rule: Swift.String, bytes: [Swift.UInt8]) throws -> CBORValue {
        let value = try decodeProfile(bytes)
        try validate(rule, value)
        return value
    }
    public static func validate(_ rule: Swift.String, _ value: CBORValue) throws {
        guard let node = GeneratedSchema.rules[rule] else { throw CBORFailure.invalid("unknown rule: " + rule) }
        // Bounds wire trees independently of potentially recursive schema traversal.
        _ = try encodeProfile(value)
        try validateNode(node, value)
    }
    internal static func validateNode(_ node: SchemaNode, _ value: CBORValue, depth: Swift.Int = 0) throws {
        // Bound transparent schema hops per value, independently of wire depth.
        // A real container edge resets this budget; encodeProfile bounds the
        // whole value tree before validation. Deep legal trees must not spend
        // their budget on repeated references/intersections at earlier levels.
        guard depth <= 512 else { throw CBORFailure.invalid("schema recursion limit") }
        func check(_ child: SchemaNode, _ v: CBORValue, containerEdge: Swift.Bool = false) throws {
            try validateNode(child, v, depth: containerEdge ? 0 : depth + 1)
        }
        func countValid(_ count: Swift.Int, _ min: Swift.UInt64, _ max: Swift.UInt64?) -> Swift.Bool {
            Swift.UInt64(count) >= min && (max == nil || Swift.UInt64(count) <= max!)
        }
        switch node {
        case .intersection(let children):
            for child in children { try check(child, value) }
        case .defaultValue(let child):
            try check(child, value)
        case .tuple(let shape):
            guard case .array(let items) = value else { throw CBORFailure.invalid("expected tuple") }
            // Dynamic programming retains every legal position, rather than
            // greedily consuming optional/repeated entries. Bound work too.
            var positions: Set<Swift.Int> = [0]
            var work = 0
            for slot in shape {
                var next: Set<Swift.Int> = []
                for start in positions {
                    let available = Swift.UInt64(items.count - start)
                    let maximum = Swift.Int(Swift.min(slot.max ?? available, available))
                    if slot.min == 0 { next.insert(start) }
                    if maximum > 0 {
                        for consumed in 1...maximum {
                            work += 1
                            guard work <= 100_000 else { throw CBORFailure.invalid("tuple work limit") }
                            guard (try? check(slot.node, items[start + consumed - 1], containerEdge: true)) != nil else { break }
                            if Swift.UInt64(consumed) >= slot.min { next.insert(start + consumed) }
                        }
                    }
                }
                positions = next
                if positions.isEmpty { break }
            }
            guard positions.contains(items.count) else { throw CBORFailure.invalid("tuple shape") }
        case .reference(let rule):
            guard let child = GeneratedSchema.rules[rule] else { throw CBORFailure.invalid("unknown reference") }
            try check(child, value)
        case .union(let items):
            for child in items { if (try? check(child, value)) != nil { return } }
            throw CBORFailure.invalid("no union branch")
        case .literal(let expected):
            guard value == expected else { throw CBORFailure.invalid("literal mismatch") }
        case .text:
            guard case .text = value else { throw CBORFailure.invalid("expected text") }
        case .boolean:
            guard case .boolean = value else { throw CBORFailure.invalid("expected boolean") }
        case .range(let low, let high):
            func lessEqual(_ a: CBORValue, _ b: CBORValue) -> Swift.Bool {
                switch (a, b) {
                case let (.unsigned(x), .unsigned(y)): return x <= y
                case let (.negative(x), .negative(y)): return x <= y
                case (.negative, .unsigned): return true
                default: return false
                }
            }
            guard lessEqual(low, value) && lessEqual(value, high) else { throw CBORFailure.invalid("integer range") }
        case .regexp(let pattern):
            guard case .text(let text) = value else { throw CBORFailure.invalid("expected text") }
            let regex = try NSRegularExpression(pattern: "\\A(?:" + pattern + ")\\z")
            let range = NSRange(text.startIndex..<text.endIndex, in: text)
            guard regex.firstMatch(in: text, range: range)?.range == range else { throw CBORFailure.invalid("text grammar") }
        case .size(let low, let high):
            guard case .text(let text) = value, Swift.UInt64(text.utf8.count) >= low, Swift.UInt64(text.utf8.count) <= high else { throw CBORFailure.invalid("text byte size") }
        case .boundedArray(let min, let max, let child):
            guard case .array(let items) = value, countValid(items.count, min, max) else { throw CBORFailure.invalid("array shape") }
            for v in items { try check(child, v, containerEdge: true) }
        case .boundedMap(let min, let max, let key, let child):
            let entries = try value.requireMap()
            guard countValid(entries.count, min, max) else { throw CBORFailure.invalid("map bounds") }
            for entry in entries { try check(key, .text(entry.key), containerEdge: true); try check(child, entry.value, containerEdge: true) }
        case .openRecord(let fields, let min, let max, let key, let child):
            let entries = try value.requireMap()
            var rest = 0
            for entry in entries {
                if let f = fields.first(where: { $0.name.utf8.elementsEqual(entry.key.utf8) }) {
                    try check(f.node, entry.value, containerEdge: true)
                } else {
                    rest += 1
                    try check(key, .text(entry.key), containerEdge: true)
                    try check(child, entry.value, containerEdge: true)
                }
            }
            guard countValid(rest, min, max) else { throw CBORFailure.invalid("record rest bounds") }
            for f in fields where !f.optional {
                guard field(entries, f.name) != nil else { throw CBORFailure.invalid("missing required field") }
            }
        case .record(let fields):
            let entries = try value.requireMap()
            for entry in entries {
                guard let f = fields.first(where: { $0.name.utf8.elementsEqual(entry.key.utf8) }) else { throw CBORFailure.invalid("unknown field") }
                try check(f.node, entry.value, containerEdge: true)
            }
            for f in fields where !f.optional {
                guard field(entries, f.name) != nil else { throw CBORFailure.invalid("missing required field") }
            }
        }
    }

    private static func header(_ major: Swift.UInt8, _ n: Swift.UInt64) -> [Swift.UInt8] {
        if n < 24 { return [major << 5 | Swift.UInt8(n)] }
        let width = n <= 0xff ? 1 : n <= 0xffff ? 2 : n <= 0xffff_ffff ? 4 : 8
        var bytes = [major << 5 | (width == 1 ? 24 : width == 2 ? 25 : width == 4 ? 26 : 27)]
        for shift in stride(from: (width - 1) * 8, through: 0, by: -8) { bytes.append(Swift.UInt8(truncatingIfNeeded: n >> shift)) }
        return bytes
    }
    public static func encodeProfile(_ value: CBORValue) throws -> [Swift.UInt8] {
        func encode(_ v: CBORValue, _ depth: Swift.Int) throws -> [Swift.UInt8] {
            try checkDepth(depth)
            switch v {
            case .unsigned(let n): return header(0, n)
            case .negative(let n):
                guard n < 0 else { throw CBORFailure.invalid("negative must be negative") }
                return header(1, Swift.UInt64(-(n + 1)))
            case .text(let s): return header(3, Swift.UInt64(s.utf8.count)) + s.utf8
            case .boolean(let b): return [b ? 0xf5 : 0xf4]
            case .null: return [0xf6]
            case .array(let items):
                var bytes = header(4, Swift.UInt64(items.count))
                for item in items { bytes += try encode(item, depth + 1) }
                return bytes
            case .map(let entries):
                var pairs: [([Swift.UInt8], [Swift.UInt8])] = []
                for e in entries { pairs.append((try encode(.text(e.key), depth + 1), try encode(e.value, depth + 1))) }
                pairs.sort { $0.0.lexicographicallyPrecedes($1.0) }
                var bytes = header(5, Swift.UInt64(pairs.count))
                var previous: [Swift.UInt8]?
                for (key, val) in pairs {
                    guard previous != key else { throw CBORFailure.invalid("duplicate key") }
                    bytes += key + val
                    previous = key
                }
                return bytes
            }
        }
        return try encode(value, 0)
    }
    public static func decodeProfile(_ bytes: [Swift.UInt8]) throws -> CBORValue {
        var pos = 0
        func byte() throws -> Swift.UInt8 {
            guard pos < bytes.count else { throw CBORFailure.invalid("truncated") }
            defer { pos += 1 }
            return bytes[pos]
        }
        func decode(_ depth: Swift.Int) throws -> CBORValue {
            try checkDepth(depth)
            let head = try byte()
            let major = head >> 5, ai = head & 31
            guard [0, 1, 3, 4, 5, 7].contains(major), ai < 28 else { throw CBORFailure.invalid("forbidden type or indefinite length") }
            if major == 7 {
                switch ai {
                case 20: return .boolean(false)
                case 21: return .boolean(true)
                case 22: return .null
                default: throw CBORFailure.invalid("forbidden simple or float")
                }
            }
            var n = Swift.UInt64(ai)
            if ai >= 24 {
                let width = 1 << Swift.Int(ai - 24)
                n = 0
                for _ in 0..<width { n = (n << 8) | Swift.UInt64(try byte()) }
                let minimum: Swift.UInt64 = width == 1 ? 24 : 1 << (8 * (width / 2))
                guard n >= minimum else { throw CBORFailure.invalid("non-shortest head") }
            }
            switch major {
            case 0: return .unsigned(n)
            case 1:
                guard n <= Swift.UInt64(Swift.Int64.max) else { throw CBORFailure.invalid("negative outside int64") }
                return .negative(-1 - Swift.Int64(n))
            case 3:
                guard n <= Swift.UInt64(bytes.count - pos) else { throw CBORFailure.invalid("truncated text") }
                let end = pos + Swift.Int(n)
                guard let s = Swift.String(bytes: bytes[pos..<end], encoding: .utf8) else { throw CBORFailure.invalid("invalid UTF-8") }
                pos = end
                return .text(s)
            case 4, 5:
                // Check against available input before converting or allocating.
                guard n <= Swift.UInt64((bytes.count - pos) / (major == 5 ? 2 : 1)) else { throw CBORFailure.invalid("truncated container") }
                if major == 4 {
                    var values: [CBORValue] = []
                    for _ in 0..<Swift.Int(n) { values.append(try decode(depth + 1)) }
                    return .array(values)
                }
                var entries: [CBORMapEntry] = []
                var previous: [Swift.UInt8]?
                for _ in 0..<Swift.Int(n) {
                    let start = pos
                    guard case .text(let key) = try decode(depth + 1) else { throw CBORFailure.invalid("nontext key") }
                    let raw = Swift.Array(bytes[start..<pos])
                    if let previous, !previous.lexicographicallyPrecedes(raw) { throw CBORFailure.invalid("duplicate or unsorted key") }
                    previous = raw
                    entries.append(.init(key, try decode(depth + 1)))
                }
                return .map(entries)
            default: throw CBORFailure.invalid("forbidden type")
            }
        }
        let value = try decode(0)
        guard pos == bytes.count else { throw CBORFailure.invalid("trailing data") }
        return value
    }
}
