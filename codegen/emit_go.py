"""Go emitter for the CDDL IR (see codegen/generate.py).

`generate(rules)` returns {repo-relative path: content}. The generated package
`sdk/surface-go/types` depends on the hand-written runtime package
`sdk/surface-go/codec` (strict CBOR profile + IR validator). Identifier
sanitization fails closed: any ambiguity raises ValueError.
"""
import json
import re
from target_ir import require_v1

GO_TYPES_PATH = 'sdk/surface-go/types/generated.go'
CODEC_IMPORT = 'github.com/softmax-universe/coglet/sdk/surface-go/codec'
UINT64_MAX = 2**64 - 1
INT64_MIN = -(2**63)

# Package-level names owned by the generated file's own scaffolding.
RESERVED_SYMBOLS = {'SchemaJSON', 'schema', 'Encode', 'Decode', 'Validate', 'codec'}
# Struct members the emitter itself defines (a field must not share the name).
RESERVED_MEMBERS = {'ToValue', 'ToCBOR'}
GO_KEYWORDS = {
    'break', 'case', 'chan', 'const', 'continue', 'default', 'defer', 'else', 'fallthrough', 'for',
    'func', 'go', 'goto', 'if', 'import', 'interface', 'map', 'package', 'range', 'return',
    'select', 'struct', 'switch', 'type', 'var',
}


def exported(name, what):
    """CDDL/JSON name -> exported Go identifier ('logical_time' -> 'LogicalTime')."""
    parts = [p for p in re.split(r'[^A-Za-z0-9]+', name) if p]
    ident = ''.join(p[0].upper() + p[1:] for p in parts)
    if not ident or not ident[0].isalpha() or not ident.isascii():
        raise ValueError(f'cannot derive a Go identifier for {what} {name!r}')
    # Exported identifiers can never be keywords; assert anyway for safety.
    assert ident not in GO_KEYWORDS
    return ident


def go_string(text):
    out = ['"']
    for ch in text:
        if ch in '\\"':
            out.append('\\' + ch)
        elif ch == '\n':
            out.append('\\n')
        elif ch == '\r':
            out.append('\\r')
        elif ch == '\t':
            out.append('\\t')
        elif ord(ch) < 0x20 or ord(ch) == 0x7f:
            out.append('\\x%02x' % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return ''.join(out)


def lower_first(name):
    return name[0].lower() + name[1:]


class Emitter:
    def __init__(self, rules):
        self.rules = rules
        self.names = {}
        self.symbols = {}
        for sym in RESERVED_SYMBOLS:
            self.symbols[sym] = 'runtime scaffolding'
        for rule in rules:
            self.names[rule] = exported(rule, 'rule')
        for rule, node in rules.items():
            name = self.names[rule]
            self.claim(name, f'type of {rule}')
            self.claim('as' + name, f'converter of {rule}')
            if self.kind(rule) != 'alias':
                self.claim('Decode' + name, f'decoder of {rule}')
            if self.kind(rule) == 'union':
                self.claim('is' + name, f'marker of {rule}')
                for i in range(len(node['items'])):
                    self.claim(f'{name}{i + 1}', f'variant {i + 1} of {rule}')
                    self.claim(f'as{name}{i + 1}', f'converter variant {i + 1} of {rule}')
                    self.claim(f'Decode{name}{i + 1}', f'decoder variant {i + 1} of {rule}')
        for rule in rules:
            if self.kind(rule) == 'alias':
                self.check_alias_cycle(rule)

    def claim(self, symbol, owner):
        if symbol in self.symbols:
            raise ValueError(f'Go symbol {symbol!r} collides: {owner} vs {self.symbols[symbol]}')
        self.symbols[symbol] = owner

    def kind(self, rule):
        node = self.rules[rule]
        if node['op'] == 'record':
            return 'record'
        if node['op'] == 'union' and all(i['op'] == 'record' for i in node['items']):
            return 'union'
        return 'alias'

    def check_alias_cycle(self, rule):
        def walk(node, stack):
            op = node['op']
            if op == 'ref':
                target = node['name']
                if self.kind(target) != 'alias':
                    return
                if target in stack:
                    raise ValueError(f'recursive Go type alias through rule {target!r}')
                walk(self.rules[target], stack | {target})
            elif op in ('array', 'map'):
                walk(node['item'], stack)
        walk(self.rules[rule], {rule})

    # --- type mapping -------------------------------------------------
    def under(self, node, stack=()):
        """Go type; refs resolved to their underlying type (for uniformity checks)."""
        return self.go_type(node, named=False, stack=stack)

    def go_type(self, node, named=True, stack=()):
        op = node['op']
        if op == 'ref':
            name = node['name']
            if named or self.kind(name) != 'alias':
                return self.names[name]
            if name in stack:
                return 'any'
            return self.go_type(self.rules[name], False, stack + (name,))
        if op in ('regexp', 'size'):
            return 'string'
        if op == 'primitive':
            return {'tstr': 'string', 'bool': 'bool', 'null': 'any'}[node['name']]
        if op == 'literal':
            value = node['value']
            if isinstance(value, bool):
                return 'bool'
            if isinstance(value, str):
                return 'string'
            if value is None:
                return 'any'
            if 0 <= value <= UINT64_MAX:
                return 'uint64'
            return 'int64' if INT64_MIN <= value < 0 else 'any'
        if op == 'range':
            if 0 <= node['min'] and node['max'] <= UINT64_MAX:
                return 'uint64'
            if INT64_MIN <= node['min'] and node['max'] < 0:
                return 'int64'
            return 'any'
        if op == 'array':
            return '[]' + self.go_type(node['item'], named, stack)
        if op == 'map':
            return 'map[string]' + self.go_type(node['item'], named, stack)
        if op == 'record':
            return 'map[string]any'
        if op == 'union':
            kinds = {self.go_type(item, False, stack) for item in node['items']}
            return kinds.pop() if len(kinds) == 1 else 'any'
        raise ValueError(f'unsupported IR op {op!r}')

    def conv(self, node):
        """Go expression: func(any) (T, error) building T from a validated value."""
        op = node['op']
        if op == 'ref':
            return 'as' + self.names[node['name']]
        gt = self.go_type(node)
        if gt == 'any':
            return 'codec.AsAny'
        if gt == 'string':
            return 'codec.AsString'
        if gt == 'bool':
            return 'codec.AsBool'
        if gt == 'uint64':
            return 'codec.AsUint64'
        if gt == 'int64':
            return 'codec.AsInt64'
        if op == 'union':
            return self.conv(node['items'][0])
        if op == 'array':
            return f'func(x any) ({gt}, error) {{ return codec.AsList(x, {self.conv(node["item"])}) }}'
        if op == 'map':
            return f'func(x any) ({gt}, error) {{ return codec.AsMap(x, {self.conv(node["item"])}) }}'
        if op == 'record':
            return 'codec.AsObject'
        raise ValueError(f'unsupported IR op {op!r}')

    @staticmethod
    def call(fn):
        return f'({fn})' if fn.startswith('func') else fn

    # --- emission -----------------------------------------------------
    def struct(self, name, rule, record, variant=None):
        fields = []
        seen = {}
        for f in record['fields']:
            ident = exported(f['name'], f'field of {rule}')
            if ident in RESERVED_MEMBERS:
                raise ValueError(f'field {f["name"]!r} of {rule} collides with generated method {ident}')
            if ident in seen:
                raise ValueError(f'fields {seen[ident]!r} and {f["name"]!r} of {rule} both map to Go field {ident}')
            seen[ident] = f['name']
            gt = self.go_type(f['type'])
            fields.append((ident, ('*' + gt) if f['optional'] else gt, f))
        out = []
        label = f'branch {variant} of CDDL rule {rule!r}' if variant else f'CDDL rule {rule!r}'
        out.append(f'// {name} is {label}. Optional fields are pointers; nil means absent.')
        if not fields:
            out.append(f'type {name} struct{{}}')
        else:
            width = max(len(i) for i, _, _ in fields)
            out.append(f'type {name} struct {{')
            out += [f'\t{i.ljust(width)} {t}' for i, t, _ in fields]
            out.append('}')
        out.append('')
        out += [f'// ToValue returns the generic CBOR value model of the record.',
                f'func (v {name}) ToValue() map[string]any {{',
                f'\tm := make(map[string]any, {len(fields)})']
        for ident, _, f in fields:
            key = go_string(f['name'])
            if f['optional']:
                out += [f'\tif v.{ident} != nil {{', f'\t\tm[{key}] = *v.{ident}', '\t}']
            else:
                out.append(f'\tm[{key}] = v.{ident}')
        out += ['\treturn m', '}', '']
        out += [f'func as{name}(x any) ({name}, error) {{', f'\tvar out {name}']
        if fields:
            out += ['\tm, ok := x.(map[string]any)', '\tif !ok {', '\t\treturn out, codec.ErrShape', '\t}',
                    '\tvar err error']
        else:
            out += ['\tif _, ok := x.(map[string]any); !ok {', '\t\treturn out, codec.ErrShape', '\t}']
        for ident, _, f in fields:
            key = go_string(f['name'])
            fn = self.call(self.conv(f['type']))
            if f['optional']:
                out += [f'\tif raw, ok := m[{key}]; ok {{', f'\t\tt, err := {fn}(raw)', '\t\tif err != nil {',
                        '\t\t\treturn out, err', '\t\t}', f'\t\tout.{ident} = &t', '\t}']
            else:
                out += [f'\tif out.{ident}, err = {fn}(m[{key}]); err != nil {{', '\t\treturn out, err', '\t}']
        out += ['\treturn out, nil', '}', '']
        return out

    def generate(self):
        ir = json.dumps(self.rules, sort_keys=True, separators=(',', ':'), ensure_ascii=False)
        out = ['// Code generated by codegen/run. DO NOT EDIT.', '', 'package types', '',
               f'import "{CODEC_IMPORT}"', '',
               '// SchemaJSON is the validation IR compiled from spec/messages/*.cddl.',
               f'const SchemaJSON = {go_string(ir)}', '',
               'var schema = codec.MustParseSchema(SchemaJSON)', '',
               '// Encode validates v against the named CDDL entry rule and returns deterministic CBOR.',
               '// v may be a generated record, or the generic value model (nil, bool, string,',
               '// integers, []any, map[string]any).',
               'func Encode(rule string, v any) ([]byte, error) {', '\treturn schema.Encode(rule, v)', '}', '',
               '// Decode strictly decodes raw, validates it against the named rule and returns',
               '// the generic value model (uint64 / int64 integers).',
               'func Decode(rule string, raw []byte) (any, error) {', '\treturn schema.Decode(rule, raw)', '}', '',
               '// Validate checks v against the named rule without returning bytes.',
               'func Validate(rule string, v any) error {', '\treturn schema.Validate(rule, v)', '}', '']
        for rule, node in self.rules.items():
            name = self.names[rule]
            kind = self.kind(rule)
            rq = go_string(rule)
            if kind == 'record':
                out += self.struct(name, rule, node)
                out += [f'// ToCBOR encodes the record as deterministic CBOR.',
                        f'func (v {name}) ToCBOR() ([]byte, error) {{', f'\treturn schema.Encode({rq}, v)', '}', '',
                        f'// Decode{name} strictly decodes and validates a {rule} item.',
                        f'func Decode{name}(raw []byte) ({name}, error) {{',
                        f'\tvar zero {name}', f'\tv, err := schema.Decode({rq}, raw)', '\tif err != nil {',
                        '\t\treturn zero, err', '\t}', f'\treturn as{name}(v)', '}', '']
            elif kind == 'union':
                out += [f'// {name} is one of the record branches of CDDL rule {rule!r}.',
                        f'type {name} interface {{', '\tcodec.Valuer', '\tToCBOR() ([]byte, error)',
                        f'\tis{name}()', '}', '']
                for i, variant in enumerate(node['items']):
                    vn = f'{name}{i + 1}'
                    out += self.struct(vn, rule, variant, i + 1)
                    out += [f'func ({vn}) is{name}() {{}}', '',
                            f'// ToCBOR encodes the record as branch {i + 1} of {rule}.',
                            f'func (v {vn}) ToCBOR() ([]byte, error) {{',
                            f'\treturn schema.EncodeVariant({rq}, {i}, v)', '}', '',
                            f'// Decode{vn} decodes an item that matches branch {i + 1} of {rule}.',
                            f'func Decode{vn}(raw []byte) ({vn}, error) {{', f'\tvar zero {vn}',
                            '\tv, err := codec.DecodeProfile(raw)', '\tif err != nil {', '\t\treturn zero, err', '\t}',
                            f'\tif err := schema.ValidateVariant({rq}, {i}, v); err != nil {{',
                            '\t\treturn zero, err', '\t}', f'\treturn as{vn}(v)', '}', '']
                out += [f'func as{name}(x any) ({name}, error) {{']
                for i in range(len(node['items'])):
                    out += [f'\tif schema.ValidateVariant({rq}, {i}, x) == nil {{',
                            f'\t\tv, err := as{name}{i + 1}(x)', '\t\tif err != nil {', '\t\t\treturn nil, err', '\t\t}',
                            '\t\treturn v, nil', '\t}']
                out += ['\treturn nil, codec.ErrShape', '}', '']
                out += [f'// Decode{name} strictly decodes and validates a {rule} item into its branch.',
                        f'func Decode{name}(raw []byte) ({name}, error) {{',
                        f'\tv, err := schema.Decode({rq}, raw)', '\tif err != nil {', '\t\treturn nil, err', '\t}',
                        f'\treturn as{name}(v)', '}', '']
            else:
                gt = self.go_type(node)
                out += [f'// {name} is CDDL rule {rule!r}; constraints are enforced by Encode/Decode.',
                        f'type {name} = {gt}', '',
                        f'func as{name}(x any) ({name}, error) {{', f'\treturn {self.call(self.conv(node))}(x)', '}', '']
        while out[-1] == '':
            out.pop()
        return '\n'.join(out) + '\n'


def generate(rules):
    """Return {repo-relative path: content} for the Go target."""
    require_v1(rules)
    return {GO_TYPES_PATH: Emitter(rules).generate()}
