import Foundation
import CogletTypes
#if canImport(CryptoKit)
import CryptoKit
#endif

/// Supervisor-provisioned authority. The digest covers the exact distribution bytes;
/// neither peer advertisements nor the artifact itself can select a contract profile.
public struct TrustedExtensionCatalog: Sendable {
    public let pin: String
    public let contractProfile: SurfaceContractProfile
    public var profile: SurfaceContractProfile { contractProfile }
    public let operations: [SurfaceOperation]
    private let rules: [String: CatalogNode]
    private let bindings: [String: CatalogBinding]
    private let schemas: [String: String]

    public init(json: [UInt8], sha256: String, contractProfile: SurfaceContractProfile = .surface01) throws {
        guard json.count <= 1_048_576 else { throw catalogError("catalog byte limit") }
        guard sha256.utf8.count == 64, sha256.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
              catalogDigest(json) == sha256 else { throw catalogError("catalog pin mismatch or absent") }
        var parser = CatalogJSON(json)
        let root = try catalogObject(parser.parse())
        try catalogKeys(root, ["version", "namespace", "rules", "schemas", "entries", "operations"])
        let namespace = try catalogText(root["namespace"])
        let reserved = ["runtime", "speech", "expression", "llm", "action", "hub", "test", "sim", "sensor", "motor", "other"]
        guard root["version"] == .unsigned(1), !namespace.isEmpty, !namespace.contains("."), !reserved.contains(namespace) else {
            throw catalogError("unsupported/reserved namespace/version")
        }
        func name(_ s: String) throws {
            try contractProfile.validateName(s)
            guard s.hasPrefix(namespace + ".") else { throw catalogError("name outside namespace") }
        }
        let rawRules = try catalogObject(root["rules"])
        guard !rawRules.isEmpty else { throw catalogError("empty rules") }
        var nodes: [String: CatalogNode] = [:]
        for (key, value) in rawRules { nodes[key] = try CatalogNode.load(value, depth: 0) }
        // Check every rule, including unused ones. Memoized graph heights avoid
        // exponential expansion of a finite but heavily shared reference DAG.
        var heights: [String: Int] = [:]
        var visiting: Set<String> = []
        func height(_ node: CatalogNode, depth: Int = 0) throws -> Int {
            guard depth <= 64 else { throw catalogError("schema nesting limit") }
            let result: Int
            switch node {
            case .reference(let key):
                guard let child = catalogLookup(nodes, key), !visiting.contains(key) else { throw catalogError("unknown/cyclic rule") }
                if let h = heights[key] { result = 1 + h }
                else {
                    visiting.insert(key)
                    let h = try height(child, depth: depth + 1)
                    visiting.remove(key); heights[key] = h; result = 1 + h
                }
            case .record(let fields): result = try fields.map { 1 + (try height($0.node, depth: depth + 1)) }.max() ?? 0
            case .union(let items), .intersection(let items): result = try items.map { 1 + (try height($0, depth: depth + 1)) }.max() ?? 0
            case .array(_, _, let child): result = 1 + (try height(child, depth: depth + 1))
            default: result = 0
            }
            guard result <= 64 else { throw catalogError("schema nesting limit") }
            return result
        }
        for key in nodes.keys {
            visiting.insert(key); heights[key] = try height(nodes[key]!); visiting.remove(key)
        }
        var localSchemas: [String: String] = [:]
        for (uri, value) in try catalogObject(root["schemas"]) {
            let rule = try catalogText(value)
            guard uri.hasPrefix("https://"), !uri.contains("coglet.dev/spec/messages/"),
                  !uri.unicodeScalars.contains(where: { CharacterSet.whitespacesAndNewlines.contains($0) }), catalogLookup(nodes, rule) != nil else {
                throw catalogError("unknown/reserved schema URI or rule")
            }
            localSchemas[uri] = rule
        }
        var entries: [String: [String]] = [:]
        for (entry, value) in try catalogObject(root["entries"]) {
            try name(entry)
            let resources = try catalogStrings(value)
            guard Set(resources).count == resources.count else { throw catalogError("duplicate entry resource") }
            for resource in resources { try name(resource) }
            entries[entry] = resources
        }
        var localBindings: [String: CatalogBinding] = [:]
        var localOperations: [SurfaceOperation] = []
        for (operation, value) in try catalogObject(root["operations"]) {
            try name(operation)
            let obj = try catalogObject(value)
            try catalogKeys(obj, ["entry", "direction", "input", "resources", "single_owner"], optional: ["output", "selection"])
            let entry = try catalogText(obj["entry"]), direction = try catalogText(obj["direction"]), input = try catalogText(obj["input"])
            let output = try catalogOptionalText(obj["output"])
            let resources = try catalogStrings(obj["resources"])
            guard obj["single_owner"] == .boolean(true), ["event", "read", "action"].contains(direction),
                  let entryResources = entries[entry], catalogLookup(localSchemas, input) != nil,
                  (direction == "event" ? output == nil : output.map { catalogLookup(localSchemas, $0) != nil } == true),
                  Set(resources).count == resources.count, resources.allSatisfy({ entryResources.contains($0) }),
                  !SurfaceOperation.builtins.contains(where: { $0.name == operation }) else {
                throw catalogError("invalid operation binding")
            }
            var selection: CatalogSelection?
            if let raw = obj["selection"], raw != .null {
                let s = try catalogObject(raw)
                try catalogKeys(s, ["operation_field", "args_field", "value_field", "reads"])
                let fields = try ["operation_field", "args_field", "value_field"].map { try catalogText(s[$0]) }
                let reads = try catalogStrings(s["reads"])
                guard direction == "event", fields.allSatisfy({ !$0.isEmpty }), Set(fields).count == 3,
                      !reads.isEmpty, Set(reads).count == reads.count else { throw catalogError("invalid selected-read binding") }
                selection = CatalogSelection(operation: fields[0], args: fields[1], value: fields[2], reads: reads)
            }
            localBindings[operation] = CatalogBinding(input: input, output: output, direction: direction, selection: selection)
            localOperations.append(SurfaceOperation(name: operation, entry: entry,
                direction: direction == "event" ? .event : (direction == "read" ? .read : .action),
                inputRule: catalogLookup(localSchemas, input)!, outputRule: output.flatMap { catalogLookup(localSchemas, $0) }, resources: resources))
        }
        guard !localOperations.isEmpty else { throw catalogError("empty operations") }
        for binding in localBindings.values {
            if let selection = binding.selection {
                guard selection.reads.allSatisfy({ localBindings[$0]?.direction == "read" }) else { throw catalogError("unknown selected read") }
            }
        }
        if let observation = localBindings["game.observation"], observation.selection == nil { throw catalogError("game.observation requires selected read") }
        self.pin = sha256; self.contractProfile = contractProfile
        self.operations = localOperations.sorted { $0.name < $1.name }
        self.rules = nodes; self.bindings = localBindings; self.schemas = localSchemas
    }

    /// Exact operation-selected schema, never a product root union or callback.
    func check(operation: String, output: Bool, value: CBORValue) throws {
        guard let binding = bindings[operation], let uri = output ? binding.output : binding.input,
              let rule = catalogLookup(schemas, uri), let node = catalogLookup(rules, rule) else { throw catalogError("unknown operation/output") }
        _ = try CBORCodec.encodeProfile(value)
        var work = 1_000_000
        try node.check(value, rules: rules, work: &work)
        if !output { _ = try selectedRead(operation: operation, value: value) }
    }

    /// Reads advertise their output URI; events/actions advertise their input URI.
    func schemaURI(operation: String) -> String {
        guard let binding = bindings[operation] else { return "" }
        return binding.direction == "read" ? (binding.output ?? "") : binding.input
    }

    /// The caller must intersect both the enclosing event grant and this read grant.
    func selectedRead(operation: String, value: CBORValue) throws -> String? {
        guard let selection = bindings[operation]?.selection else { return nil }
        let fields = try catalogObject(value)
        let read = try catalogText(fields[selection.operation])
        guard selection.reads.contains(read), let args = fields[selection.args], let result = fields[selection.value] else {
            throw catalogError("unsupported/missing selected read")
        }
        try check(operation: read, output: false, value: args)
        try check(operation: read, output: true, value: result)
        return read
    }
}

private func catalogError(_ message: String) -> CBORFailure { .invalid("trusted catalog: " + message) }
// Swift Dictionary<String,...> normalizes Unicode equality. Catalog identities
// are exact distribution bytes, just like Rust BTreeMap and CBOR map keys.
private func catalogLookup<T>(_ values: [String: T], _ key: String) -> T? {
    values.first { $0.key.utf8.elementsEqual(key.utf8) }?.value
}
private func catalogObject(_ value: CBORValue?) throws -> [String: CBORValue] {
    guard case .map(let entries) = value else { throw catalogError("expected object") }
    var result: [String: CBORValue] = [:]
    for entry in entries {
        guard result[entry.key] == nil else { throw catalogError("duplicate/normalization-colliding key") }
        result[entry.key] = entry.value
    }
    return result
}
private func catalogKeys(_ object: [String: CBORValue], _ required: Set<String>, optional: Set<String> = []) throws {
    guard required.isSubset(of: Set(object.keys)), Set(object.keys).isSubset(of: required.union(optional)) else { throw catalogError("unknown/missing member") }
}
private func catalogText(_ value: CBORValue?) throws -> String {
    guard case .text(let text) = value else { throw catalogError("expected string") }; return text
}
private func catalogOptionalText(_ value: CBORValue?) throws -> String? {
    if value == nil || value == .null { return nil }; return try catalogText(value)
}
private func catalogStrings(_ value: CBORValue?) throws -> [String] {
    guard case .array(let items) = value else { throw catalogError("expected array") }; return try items.map { try catalogText($0) }
}
private func catalogLessEqual(_ a: CBORValue, _ b: CBORValue) -> Bool {
    switch (a, b) {
    case let (.unsigned(x), .unsigned(y)): return x <= y
    case let (.negative(x), .negative(y)): return x <= y
    case (.negative, .unsigned): return true
    default: return false
    }
}
private func catalogInteger(_ value: CBORValue?) throws -> CBORValue {
    switch value {
    case .unsigned(let n): return .unsigned(n)
    case .negative(let n) where n < 0: return .negative(n)
    default: throw catalogError("expected integer")
    }
}
private func catalogUnsigned(_ value: CBORValue?) throws -> UInt64 {
    guard case .unsigned(let n) = value else { throw catalogError("expected unsigned integer") }; return n
}
private struct CatalogBinding: Sendable { let input: String; let output: String?; let direction: String; let selection: CatalogSelection? }
private struct CatalogSelection: Sendable { let operation: String; let args: String; let value: String; let reads: [String] }
private struct CatalogField: Sendable { let name: String; let optional: Bool; let node: CatalogNode }
private indirect enum CatalogNode: Sendable {
    case reference(String), record([CatalogField]), union([CatalogNode]), intersection([CatalogNode])
    case array(UInt64, UInt64, CatalogNode), text, boolean, null, range(CBORValue, CBORValue), size(UInt64, UInt64), literal(CBORValue), regexp(CatalogPattern)

    static func load(_ value: CBORValue, depth: Int) throws -> CatalogNode {
        guard depth <= 64 else { throw catalogError("schema nesting limit") }
        let o = try catalogObject(value), op = try catalogText(o["op"])
        func child(_ value: CBORValue?) throws -> CatalogNode {
            guard let value else { throw catalogError("missing child") }; return try load(value, depth: depth + 1)
        }
        switch op {
        case "ref": try catalogKeys(o, ["op", "name"]); return .reference(try catalogText(o["name"]))
        case "record":
            try catalogKeys(o, ["op", "fields"])
            guard case .array(let raw) = o["fields"] else { throw catalogError("expected fields") }
            var seen: Set<String> = []
            let fields = try raw.map { value -> CatalogField in
                let f = try catalogObject(value); try catalogKeys(f, ["name", "optional", "type"])
                let name = try catalogText(f["name"])
                guard case .boolean(let optional) = f["optional"], seen.insert(name).inserted else { throw catalogError("invalid/duplicate field") }
                return CatalogField(name: name, optional: optional, node: try child(f["type"]))
            }
            return .record(fields)
        case "union", "intersection":
            try catalogKeys(o, ["op", "items"])
            guard case .array(let items) = o["items"], !items.isEmpty else { throw catalogError("empty alternatives") }
            let children = try items.map { try child($0) }
            return op == "union" ? .union(children) : .intersection(children)
        case "array":
            try catalogKeys(o, ["op", "min", "max", "item"])
            let lo = try catalogUnsigned(o["min"]), hi = try catalogUnsigned(o["max"])
            guard lo <= hi, hi <= 262_144 else { throw catalogError("unbounded array") }
            return .array(lo, hi, try child(o["item"]))
        case "primitive":
            try catalogKeys(o, ["op", "name"])
            switch try catalogText(o["name"]) {
            case "tstr": return .text
            case "bool": return .boolean
            case "null": return .null
            default: throw catalogError("unknown primitive")
            }
        case "range":
            try catalogKeys(o, ["op", "min", "max"])
            let lo = try catalogInteger(o["min"]), hi = try catalogInteger(o["max"])
            guard catalogLessEqual(lo, hi) else { throw catalogError("invalid bounds") }; return .range(lo, hi)
        case "size":
            try catalogKeys(o, ["op", "min", "max"])
            let lo = try catalogUnsigned(o["min"]), hi = try catalogUnsigned(o["max"])
            guard lo <= hi else { throw catalogError("invalid bounds") }; return .size(lo, hi)
        case "literal":
            try catalogKeys(o, ["op", "value"])
            guard let v = o["value"] else { throw catalogError("missing literal") }
            _ = try CBORCodec.encodeProfile(v); return .literal(v)
        case "regexp":
            try catalogKeys(o, ["op", "pattern"]); return .regexp(try CatalogPattern(catalogText(o["pattern"])))
        default: throw catalogError("unsupported/open IR")
        }
    }

    func check(_ value: CBORValue, rules: [String: CatalogNode], work: inout Int) throws {
        work -= 1; guard work >= 0 else { throw catalogError("validation work limit") }
        switch self {
        case .reference(let key):
            guard let child = catalogLookup(rules, key) else { throw catalogError("unknown rule") }; try child.check(value, rules: rules, work: &work)
        case .record(let fields):
            guard case .map(let entries) = value else { throw catalogError("expected record") }
            for entry in entries {
                guard let f = fields.first(where: { $0.name.utf8.elementsEqual(entry.key.utf8) }) else { throw catalogError("unknown record field") }
                try f.node.check(entry.value, rules: rules, work: &work)
            }
            for f in fields where !f.optional {
                guard entries.contains(where: { $0.key.utf8.elementsEqual(f.name.utf8) }) else { throw catalogError("missing record field") }
            }
        case .union(let children):
            for child in children { if (try? child.check(value, rules: rules, work: &work)) != nil { return } }
            throw catalogError("no union branch")
        case .intersection(let children): for child in children { try child.check(value, rules: rules, work: &work) }
        case .array(let lo, let hi, let child):
            guard case .array(let items) = value, UInt64(items.count) >= lo, UInt64(items.count) <= hi else { throw catalogError("array bounds") }
            for item in items { try child.check(item, rules: rules, work: &work) }
        case .text: guard case .text = value else { throw catalogError("expected text") }
        case .boolean: guard case .boolean = value else { throw catalogError("expected bool") }
        case .null: guard value == .null else { throw catalogError("expected null") }
        case .range(let lo, let hi): guard catalogLessEqual(lo, value), catalogLessEqual(value, hi) else { throw catalogError("integer bounds") }
        case .size(let lo, let hi):
            guard case .text(let text) = value, UInt64(text.utf8.count) >= lo, UInt64(text.utf8.count) <= hi else { throw catalogError("text byte bounds") }
        case .literal(let expected): guard catalogEqual(expected, value) else { throw catalogError("literal mismatch") }
        case .regexp(let pattern):
            guard case .text(let text) = value else { throw catalogError("expected text") }; try pattern.check(text, work: &work)
        }
    }
}

// JSON objects and CBOR maps are unordered for literal comparison, but text is
// compared as exact UTF-8, not Swift's canonically equivalent String equality.
private func catalogEqual(_ a: CBORValue, _ b: CBORValue) -> Bool {
    switch (a, b) {
    case let (.map(x), .map(y)):
        return x.count == y.count && x.allSatisfy { e in y.contains { $0.key.utf8.elementsEqual(e.key.utf8) && catalogEqual(e.value, $0.value) } }
    case let (.array(x), .array(y)): return x.count == y.count && zip(x, y).allSatisfy { catalogEqual($0.0, $0.1) }
    default: return a == b
    }
}

/// Deliberately portable regexp subset: ASCII literals/classes, concatenation,
/// and occurrence bounds, plus the generated Unicode-scalar [\s\S] class.
/// Reject groups, alternation, backreferences, Unicode properties, dot, and
/// dialect-sensitive escapes instead of interpreting them with ICU semantics.
private struct CatalogPattern: Sendable {
    struct Atom: Sendable { let ranges: [ClosedRange<UInt32>]; let any: Bool; var min: UInt64; var max: UInt64 }
    let atoms: [Atom]
    init(_ pattern: String) throws {
        var bytes = Array(pattern.utf8)
        if bytes.first == 94 { bytes.removeFirst() }; if bytes.last == 36 { bytes.removeLast() }
        var i = 0; var result: [Atom] = []
        func fail() -> CBORFailure { catalogError("unsupported/nonportable regexp") }
        func number() throws -> UInt64 {
            let start = i
            while i < bytes.count, (48...57).contains(bytes[i]) { i += 1 }
            guard i > start, let n = UInt64(String(decoding: bytes[start..<i], as: UTF8.self)) else { throw fail() }; return n
        }
        while i < bytes.count {
            var ranges: [ClosedRange<UInt32>] = []; var any = false
            if bytes[i...].starts(with: Array("[\\s\\S]".utf8)) { any = true; i += 6 }
            else if bytes[i] == 91 {
                i += 1
                while i < bytes.count, bytes[i] != 93 {
                    let lo = bytes[i]
                    guard lo >= 32, lo < 127, ![92, 91, 94, 45].contains(lo) else { throw fail() }
                    i += 1; var hi = lo
                    if i < bytes.count, bytes[i] == 45 {
                        i += 1; guard i < bytes.count else { throw fail() }; hi = bytes[i]; i += 1
                        guard hi >= lo, hi < 127, ![92, 91, 93].contains(hi) else { throw fail() }
                    }
                    ranges.append(UInt32(lo)...UInt32(hi))
                }
                guard i < bytes.count, !ranges.isEmpty else { throw fail() }; i += 1
            } else {
                var byte = bytes[i]; i += 1
                if byte == 92 {
                    guard i < bytes.count, Array(".[]{}()*+?|^$\\-".utf8).contains(bytes[i]) else { throw fail() }
                    byte = bytes[i]; i += 1
                } else if Array(".[]{}()*+?|^$".utf8).contains(byte) { throw fail() }
                guard byte >= 32, byte < 127 else { throw fail() }; ranges = [UInt32(byte)...UInt32(byte)]
            }
            var lo: UInt64 = 1, hi: UInt64 = 1
            if i < bytes.count {
                switch bytes[i] {
                case 42: lo = 0; hi = UInt64.max; i += 1
                case 43: hi = UInt64.max; i += 1
                case 63: lo = 0; i += 1
                case 123:
                    i += 1; lo = try number(); hi = lo
                    if i < bytes.count, bytes[i] == 44 { i += 1; hi = try number() }
                    guard i < bytes.count, bytes[i] == 125, lo <= hi else { throw fail() }; i += 1
                default: break
                }
            }
            result.append(Atom(ranges: ranges, any: any, min: lo, max: hi))
        }
        atoms = result
    }
    func check(_ text: String, work: inout Int) throws {
        let scalars = text.unicodeScalars.map(\.value)
        var positions: Set<Int> = [0]
        for atom in atoms {
            var next: Set<Int> = []
            for start in positions {
                if atom.min == 0 { next.insert(start) }
                let maximum = Int(min(atom.max, UInt64(scalars.count - start)))
                if maximum > 0 {
                    for length in 1...maximum {
                        work -= 1; guard work >= 0 else { throw catalogError("regexp work limit") }
                        let scalar = scalars[start + length - 1]
                        guard atom.any || atom.ranges.contains(where: { $0.contains(scalar) }) else { break }
                        if UInt64(length) >= atom.min { next.insert(start + length) }
                    }
                }
            }
            positions = next
            if positions.isEmpty { break }
        }
        guard positions.contains(scalars.count) else { throw catalogError("regexp mismatch") }
    }
}

/// Exact JSON grammar with integer-only numbers (the closed IR/CBOR profile has
/// no floats), strict UTF-8, duplicate keys at every depth and no trailing tokens.
private struct CatalogJSON {
    let bytes: [UInt8]; var index = 0
    init(_ bytes: [UInt8]) { self.bytes = bytes }
    mutating func parse() throws -> CBORValue {
        guard String(bytes: bytes, encoding: .utf8) != nil else { throw catalogError("invalid UTF-8") }
        let result = try value(0); whitespace()
        guard index == bytes.count else { throw catalogError("trailing JSON tokens") }; return result
    }
    mutating func whitespace() { while index < bytes.count, [9, 10, 13, 32].contains(bytes[index]) { index += 1 } }
    mutating func take(_ byte: UInt8) -> Bool {
        whitespace(); guard index < bytes.count, bytes[index] == byte else { return false }; index += 1; return true
    }
    mutating func string() throws -> String {
        whitespace(); let start = index
        guard take(34) else { throw catalogError("expected JSON string") }
        while index < bytes.count {
            let byte = bytes[index]; index += 1
            if byte == 34 {
                // JSONDecoder validates escapes and surrogate pairing; it never
                // sees objects, so duplicate-key handling is ours alone.
                return try JSONDecoder().decode(String.self, from: Data(bytes[start..<index]))
            }
            guard byte >= 32 else { throw catalogError("JSON control character") }
            if byte == 92 {
                guard index < bytes.count else { throw catalogError("truncated escape") }; index += 1
            }
        }
        throw catalogError("unterminated JSON string")
    }
    mutating func value(_ depth: Int) throws -> CBORValue {
        guard depth <= 256 else { throw catalogError("JSON nesting limit") }
        whitespace(); guard index < bytes.count else { throw catalogError("truncated JSON") }
        switch bytes[index] {
        case 34: return .text(try string())
        case 123:
            index += 1; var entries: [CBORMapEntry] = []; var seen: Set<String> = []
            if take(125) { return .map(entries) }
            repeat {
                let key = try string()
                guard seen.insert(key).inserted, take(58) else { throw catalogError("duplicate key/missing colon") }
                entries.append(CBORMapEntry(key, try value(depth + 1)))
                if take(125) { return .map(entries) }
            } while take(44)
            throw catalogError("JSON object delimiter")
        case 91:
            index += 1; var items: [CBORValue] = []
            if take(93) { return .array(items) }
            repeat {
                items.append(try value(depth + 1)); if take(93) { return .array(items) }
            } while take(44)
            throw catalogError("JSON array delimiter")
        case 116, 102, 110:
            let word = bytes[index] == 116 ? "true" : (bytes[index] == 102 ? "false" : "null")
            guard bytes[index...].starts(with: word.utf8) else { throw catalogError("JSON literal") }; index += word.utf8.count
            return word == "null" ? .null : .boolean(word == "true")
        default:
            let start = index
            if bytes[index] == 45 { index += 1 }
            guard index < bytes.count, (48...57).contains(bytes[index]) else { throw catalogError("JSON number") }
            if bytes[index] == 48 { index += 1 }
            else { while index < bytes.count, (48...57).contains(bytes[index]) { index += 1 } }
            let token = String(decoding: bytes[start..<index], as: UTF8.self)
            if token == "-0" { return .unsigned(0) }
            if token.hasPrefix("-"), let n = Int64(token), n < 0 { return .negative(n) }
            if !token.hasPrefix("-"), let n = UInt64(token) { return .unsigned(n) }
            throw catalogError("JSON integer overflow")
        }
    }
}

// CryptoKit is present on supported Apple platforms. A small SHA-256 fallback
// keeps the dependency-free package buildable under Linux Swift as well.
func catalogDigest(_ bytes: [UInt8]) -> String {
#if canImport(CryptoKit)
    return SHA256.hash(data: Data(bytes)).map { String(format: "%02x", $0) }.joined()
#else
    let constants: [UInt32] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2]
    var hash: [UInt32] = [0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]
    var padded = bytes; let bits = UInt64(bytes.count) * 8
    padded.append(128); while padded.count % 64 != 56 { padded.append(0) }
    for shift in stride(from: 56, through: 0, by: -8) { padded.append(UInt8(truncatingIfNeeded: bits >> shift)) }
    func rotate(_ x: UInt32, _ n: UInt32) -> UInt32 { (x >> n) | (x << (32 - n)) }
    for offset in stride(from: 0, to: padded.count, by: 64) {
        var w = [UInt32](repeating: 0, count: 64)
        for i in 0..<16 {
            let j = offset + i * 4
            w[i] = UInt32(padded[j]) << 24 | UInt32(padded[j+1]) << 16 | UInt32(padded[j+2]) << 8 | UInt32(padded[j+3])
        }
        for i in 16..<64 {
            let x = w[i-15], y = w[i-2]
            let s0 = rotate(x,7) ^ rotate(x,18) ^ (x >> 3), s1 = rotate(y,17) ^ rotate(y,19) ^ (y >> 10)
            w[i] = w[i-16] &+ s0 &+ w[i-7] &+ s1
        }
        var a=hash[0], b=hash[1], c=hash[2], d=hash[3], e=hash[4], f=hash[5], g=hash[6], h=hash[7]
        for i in 0..<64 {
            let s1 = rotate(e,6) ^ rotate(e,11) ^ rotate(e,25), ch = (e & f) ^ (~e & g)
            let t1 = h &+ s1 &+ ch &+ constants[i] &+ w[i]
            let s0 = rotate(a,2) ^ rotate(a,13) ^ rotate(a,22), maj = (a & b) ^ (a & c) ^ (b & c)
            let t2 = s0 &+ maj
            h=g; g=f; f=e; e=d &+ t1; d=c; c=b; b=a; a=t1 &+ t2
        }
        let values = [a,b,c,d,e,f,g,h]
        for i in 0..<8 { hash[i] = hash[i] &+ values[i] }
    }
    return hash.map { String(format: "%08x", $0) }.joined()
#endif
}
