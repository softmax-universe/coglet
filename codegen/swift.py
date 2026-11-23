"""Swift value types and native validation IR, derived only from the CDDL IR."""
import json
import re


def quote(value):
    # JSON escapes are not Swift escapes (notably Unicode and escaped slash).
    return '"' + ''.join({'"': '\\"', '\\': '\\\\', '\n': '\\n', '\r': '\\r', '\t': '\\t'}.get(c, c if 32 <= ord(c) < 127 else '\\u{' + format(ord(c), 'x') + '}') for c in value) + '"'


def name(value):
    parts = re.split('[-_:]+', value)
    return ''.join(p[:1].upper() + p[1:] for p in parts)


def field_name(value):
    value = value.replace('-', '_')
    if not re.fullmatch('[A-Za-z_][A-Za-z0-9_]*', value) or value == '_':
        raise ValueError(f'unsupported Swift field identifier: {value}')
    # Escaping every name covers contextual keywords, including future ones.
    if value in ('self', 'Self', 'super', 'init', 'deinit', 'subscript', 'Type', 'Protocol', 'cborValue', 'schemaRule', 'toCBOR', 'fromCBOR', 'depth', 'Swift', 'Foundation', 'CBORCodec', 'CBORValue'):
        value += '_'
    return '`' + value + '`'


def generate_swift(rules):
    from target_ir import require_v1
    require_v1(rules)
    definitions = dict(rules)
    names = {}
    reserved = {'Swift', 'Foundation', 'Any', 'Set', 'Equatable', 'Sendable', 'CBORValue', 'CBORMapEntry', 'CBORCodec', 'CBORConvertible', 'CBORMessage', 'CBORFailure', 'SchemaNode', 'SchemaField', 'SchemaTupleItem', 'GeneratedSchema', 'NSRegularExpression', 'NSRange'}

    def register(key, swift_name):
        if swift_name in reserved or swift_name in names.values():
            raise ValueError(f'Swift type identifier collision: {key} -> {swift_name}')
        if not re.fullmatch('[A-Za-z][A-Za-z0-9]*', swift_name) or swift_name in ('Self', 'Type', 'Protocol'):
            raise ValueError(f'unsupported Swift type identifier: {swift_name}')
        names[key] = swift_name

    for key in rules:
        register(key, name(key))

    def references(node, layout=False):
        op = node['op']
        if op == 'ref':
            return {node['name']}
        if op == 'union':
            return set() if layout else set().union(*(references(v) for v in node['items']))
        if op == 'default':
            return references(node['item'], layout)
        if op == 'record' and layout and 'rest' not in node:
            return set().union(*(references(f['type'], True) for f in node['fields']))
        return set()

    # Transparent recursive aliases/unions consume no wire depth and cannot
    # choose a productive typed branch. Reject them rather than looping during
    # conversion. Struct-only recursion also has no finite Swift value layout;
    # recursive arrays/maps/indirect unions remain supported.
    for layout in (False, True):
        active, done = set(), set()
        def visit(key):
            if key in active:
                raise ValueError('unsupported Swift recursive type cycle: ' + key)
            if key in done:
                return
            active.add(key)
            for ref in references(rules[key], layout):
                visit(ref)
            active.remove(key)
            done.add(key)
        for key in rules:
            visit(key)

    # Collection aliases do not introduce a nominal Swift type. Recursion must
    # cross a struct/indirect enum, not just Array's indirection in its storage.
    def alias_references(node):
        if node['op'] == 'ref':
            return {node['name']}
        if node['op'] in ('array', 'default'):
            return alias_references(node['item'])
        return set()
    active, done = set(), set()
    def visit_alias(key):
        if key in active:
            raise ValueError('unsupported Swift recursive type cycle: ' + key)
        if key in done:
            return
        active.add(key)
        for ref in alias_references(rules[key]):
            visit_alias(ref)
        active.remove(key)
        done.add(key)
    for key in rules:
        visit_alias(key)

    def typ(node, hint):
        op = node['op']
        if op in ('intersection', 'tuple'):
            return 'CBORValue' if op == 'intersection' else '[CBORValue]'
        if op == 'default':
            return typ(node['item'], hint + 'Default')
        if op == 'record' and 'rest' in node:
            return '[CBORMapEntry]'
        if op == 'ref':
            return names[node['name']]
        if op in ('record', 'union'):
            key = '@' + hint
            if key not in definitions:
                register(key, hint)
                definitions[key] = node
            return names[key]
        if op in ('regexp', 'size') or op == 'primitive' and node['name'] == 'tstr':
            return 'Swift.String'
        if op == 'primitive':
            return 'CBORValue' if node['name'] == 'null' else 'Swift.Bool'
        if op == 'range':
            return 'Swift.UInt64' if node['min'] >= 0 else 'Swift.Int64' if node['max'] <= 2**63 - 1 else 'CBORValue'
        if op == 'literal':
            v = node['value']
            return 'CBORValue' if v is None else 'Swift.Bool' if type(v) is bool else 'Swift.String' if isinstance(v, str) else 'Swift.Int64' if v < 0 else 'Swift.UInt64'
        if op == 'array':
            return '[' + typ(node['item'], hint + 'Item') + ']'
        if op == 'map':
            # Native entries preserve byte-distinct canonically equivalent text keys.
            return '[CBORMapEntry]'
        raise ValueError('unsupported Swift op ' + op)

    def literal(v):
        if v is None:
            return '.null'
        if type(v) is bool:
            return '.boolean(' + str(v).lower() + ')'
        if isinstance(v, str):
            return '.text(' + quote(v) + ')'
        return ('.negative(' if v < 0 else '.unsigned(') + str(v) + ')'

    def schema(n):
        op = n['op']
        def bound(value):
            return 'nil' if value is None else str(value)
        def fields():
            return '[' + ', '.join('.init(name: ' + quote(f['name']) + ', optional: ' + str(f['optional']).lower() + ', node: ' + schema(f['type']) + ')' for f in n['fields']) + ']'
        if op == 'intersection':
            return '.intersection([' + ', '.join(map(schema, n['items'])) + '])'
        if op == 'default':
            # Default is metadata, never a wire insertion or type broadening.
            return '.defaultValue(' + schema(n['item']) + ')'
        if op == 'tuple':
            return '.tuple([' + ', '.join(f'.init(min: {v["min"]}, max: {bound(v.get("max"))}, node: {schema(v["type"])})' for v in n['items']) + '])'
        if op == 'ref':
            return '.reference(' + quote(n['name']) + ')'
        if op == 'literal':
            return '.literal(' + literal(n['value']) + ')'
        if op == 'primitive':
            return '.text' if n['name'] == 'tstr' else '.literal(.null)' if n['name'] == 'null' else '.boolean'
        if op == 'range':
            return '.range(' + literal(n['min']) + ', ' + literal(n['max']) + ')'
        if op == 'regexp':
            return '.regexp(' + quote(n['pattern']) + ')'
        if op == 'size':
            return f'.size({n["min"]}, {n["max"]})'
        if op == 'array':
            return f'.boundedArray({n["min"]}, {bound(n.get("max"))}, {schema(n["item"])})'
        if op == 'map':
            return f'.boundedMap({n.get("min", 0)}, {bound(n.get("max"))}, {schema(n.get("key", {"op": "primitive", "name": "tstr"}))}, {schema(n["item"])})'
        if op == 'union':
            return '.union([' + ', '.join(map(schema, n['items'])) + '])'
        if op == 'record':
            if 'rest' in n:
                rest = n['rest']
                return f'.openRecord({fields()}, {rest["min"]}, {bound(rest.get("max"))}, {schema(rest.get("key", {"op": "primitive", "name": "tstr"}))}, {schema(rest["item"])})'
            return '.record(' + fields() + ')'
        raise ValueError(op)

    out = ['// Generated by codegen/run. DO NOT EDIT.', 'import Foundation', '', 'internal enum GeneratedSchema {', '    static let rules: [Swift.String: SchemaNode] = [']
    out += ['        ' + quote(k) + ': ' + schema(v) + ',' for k, v in rules.items()]
    out += ['    ]', '}', '']
    index = 0
    while index < len(definitions):
        key = list(definitions)[index]
        index += 1
        node, cls = definitions[key], names[key]
        op = node['op']
        if op not in ('record', 'union') or op == 'record' and 'rest' in node:
            out += [f'public typealias {cls} = {typ(node, cls)}', '']
            continue
        out += [f'public {"indirect enum" if op == "union" else "struct"} {cls}: CBORMessage, Sendable {{', f'    public static let schemaRule = {quote(key)}']
        # Inline nodes also have their own validation rule.
        if key.startswith('@'):
            out[-1] = '    public static let schemaRule = ' + quote(key)
        if op == 'record':
            fields = node['fields']
            ids = [field_name(f['name']) for f in fields]
            if len(set(ids)) != len(ids):
                raise ValueError('Swift field identifier collision in ' + key)
            types = [typ(f['type'], cls + name(f['name'])) for f in fields]
            for f, ident, t in zip(fields, ids, types):
                out += [f'    public var {ident}: {t}' + ('?' if f['optional'] else '')]
            args = ', '.join(f'{i}: {t}' + ('? = nil' if f['optional'] else '') for f, i, t in zip(fields, ids, types))
            out += [f'    public init({args}) {{']
            out += [f'        self.{i} = {i}' for i in ids]
            out += ['    }', '    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {', '        try CBORCodec.checkDepth(depth)', f'        try CBORCodec.validate(Self.schemaRule, cborValue)', '        let entries = try cborValue.requireMap()']
            for f, i, t in zip(fields, ids, types):
                lookup = 'CBORCodec.field(entries, ' + quote(f['name']) + ')'
                if f['optional']:
                    out += [f'        self.{i} = try {lookup}.map {{ try {t}.init(cborValue: $0, depth: depth + 1) }}']
                else:
                    out += [f'        self.{i} = try {t}.init(cborValue: CBORCodec.required({lookup}), depth: depth + 1)']
            out += ['    }', '    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {', '        try CBORCodec.checkDepth(depth)', '        var entries: [CBORMapEntry] = []']
            for f, i in zip(fields, ids):
                if f['optional']:
                    out += [f'        if let v = self.{i} {{ entries.append(.init({quote(f["name"])}, try v.cborValue(depth: depth + 1))) }}']
                else:
                    out += [f'        entries.append(.init({quote(f["name"])}, try self.{i}.cborValue(depth: depth + 1)))']
            out += ['        let value = CBORValue.map(entries)', '        try CBORCodec.validate(Self.schemaRule, value)', '        return value', '    }']
        else:
            types = [typ(v, cls + 'Variant' + str(i + 1)) for i, v in enumerate(node['items'])]
            out += [f'    case variant{i + 1}({t})' for i, t in enumerate(types)]
            out += ['    public init(cborValue: CBORValue, depth: Swift.Int = 0) throws {', '        try CBORCodec.checkDepth(depth)']
            for i, (v, t) in enumerate(zip(node['items'], types)):
                out += [f'        if (try? CBORCodec.validateNode({schema(v)}, cborValue)) != nil {{', f'            self = .variant{i+1}(try {t}.init(cborValue: cborValue, depth: depth))', '            return', '        }']
            out += ['        throw CBORFailure.invalid("no union branch matched")', '    }', '    public func cborValue(depth: Swift.Int = 0) throws -> CBORValue {', '        try CBORCodec.checkDepth(depth)', '        let value: CBORValue', '        switch self {']
            for i, branch in enumerate(node['items']):
                out += [f'        case .variant{i+1}(let v):', '            value = try v.cborValue(depth: depth)', f'            try CBORCodec.validateNode({schema(branch)}, value)']
            out += ['        }', '        try CBORCodec.validate(Self.schemaRule, value)', '        return value', '    }']
        out += ['}', '']
    # All inline types are now known, including those emitted after the record
    # that contains a shadowing field. Backticks cannot disambiguate expression
    # constructors such as Child.init from a stored property named Child.
    for key, node in definitions.items():
        if node['op'] == 'record' and 'rest' not in node:
            for field in node['fields']:
                if field_name(field['name']).strip('`') in names.values():
                    raise ValueError('Swift field/type identifier collision in ' + key + ': ' + field['name'])
    # Include inline record/union rules in the same schema table.
    start = out.index('    ]')
    out[start:start] = ['        ' + quote(k) + ': ' + schema(v) + ',' for k, v in definitions.items() if k.startswith('@')]
    end = out.index('}')
    roundtrip = ['    static func typedRoundTrip(rule: Swift.String, bytes: [Swift.UInt8]) throws -> [Swift.UInt8] {', '        let value = try CBORCodec.decode(rule: rule, bytes: bytes)', '        let typed: CBORValue', '        switch rule {']
    for key, node in rules.items():
        roundtrip += ['        case ' + quote(key) + ': typed = try ' + names[key] + '.init(cborValue: value, depth: 0).cborValue(depth: 0)']
    roundtrip += ['        default: throw CBORFailure.invalid("unknown typed rule")', '        }', '        return try CBORCodec.encode(rule: rule, value: typed)', '    }']
    out[end:end] = roundtrip
    return '\n'.join(out)
