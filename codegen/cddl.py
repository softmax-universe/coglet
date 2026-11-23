"""Fail-closed compiler for the inventoried RFC 8610 subset.

Groups are compile-time only: splice and distribute choices before emission.
Namespacing is a compiler API convention (file::rule), not new CDDL syntax.
"""
import copy
import json
import re

TOKEN = re.compile(r'\s+|;[^\n]*|"(?:[^"\\]|\\.)*"|-?[0-9]+|\$\$?[A-Za-z_][A-Za-z0-9_-]*|[A-Za-z_][A-Za-z0-9_-]*|\.[A-Za-z]+|\.\.|//=|/=|//|=>|[{}\[\](),:?+*/=]')
NAME = re.compile(r'(?:\$\$?)?[A-Za-z_][A-Za-z0-9_-]*\Z')


def choice(items):
    return items[0] if len(items) == 1 else {'op': 'union', 'items': items}


def bounds(lo, hi):
    if lo < 0 or hi is not None and hi < lo:
        raise ValueError(f'invalid occurrence/size bounds {lo}..{hi}')
    return dict(min=lo, **({} if hi is None else {'max': hi}))


class Parser:
    def __init__(self, text):
        self.tokens = []
        self.spaced = []  # spaced[k]: token k is preceded by whitespace/comment
        gap = False
        pos = 0
        while pos < len(text):
            m = TOKEN.match(text, pos)
            if not m:
                raise ValueError(f'unsupported CDDL construct at {text[pos:pos+40]!r}')
            t = m.group()
            if t.isspace() or t.startswith(';'):
                gap = True
            else:
                self.tokens.append(t)
                self.spaced.append(gap)
                gap = False
            pos = m.end()
        self.i = 0

    def peek(self, offset=0):
        return (self.tokens + [None] * (offset + 1))[self.i + offset]

    def take(self, expected=None):
        t = self.peek()
        if t is None:
            raise ValueError('unexpected end of CDDL')
        if expected is not None and t != expected:
            raise ValueError(f'expected {expected!r}, got {t!r} (unsupported CDDL construct)')
        self.i += 1
        return t

    def expr(self):
        items = [self.control()]
        while self.peek() == '/':
            self.take()
            items.append(self.control())
        return choice(items)

    def control(self):
        node = self.atom()
        while self.peek() and self.peek().startswith('.') and self.peek() != '..':
            op = self.take()
            if op == '.and':
                node = {'op': 'intersection', 'items': [node, self.atom()]}
            elif op == '.default':
                val = self.atom()
                if val['op'] != 'literal':
                    raise ValueError('unsupported .default: expected literal')
                node = {'op': 'default', 'item': node, 'value': val['value']}
            elif op == '.regexp':
                pattern = json.loads(self.take())
                # Portable ASCII regexp subset: reject engine-specific groups,
                # backreferences and character classes with different semantics.
                portable_pattern = pattern.replace(r'[\s\S]', '.')
                if re.search(r'\(\?(?!:)|\\[1-9dDsSwW]|\\[pP]', portable_pattern):
                    raise ValueError(f'unsupported .regexp construct: {pattern!r}')
                try:
                    re.compile(pattern)
                except re.error as e:
                    raise ValueError(f'invalid .regexp: {e}') from e
                node = {'op': 'intersection', 'items': [node, {'op': 'regexp', 'pattern': pattern}]}
            elif op == '.size':
                size = self.atom()
                if size['op'] == 'literal' and type(size['value']) is int:
                    low = high = size['value']
                elif size['op'] == 'range':
                    low, high = size['min'], size['max']
                else:
                    raise ValueError('unsupported .size operand')
                node = {'op': 'intersection', 'items': [node, {'op': 'size', **bounds(low, high)}]}
            else:
                raise ValueError(f'unsupported CDDL control {op}')
        return node

    def adjacent(self, offset):
        """True when token at offset touches the previous token (RFC 8610 occur has no inner whitespace)."""
        k = self.i + offset
        return k < len(self.tokens) and not self.spaced[k]

    def bound_digit(self, offset):
        t = self.peek(offset)
        if not (t or '').isdigit():
            return False
        if not self.adjacent(offset):
            # `* 64 x` / `1 * x`: RFC 8610 `occur = [uint] "*" [uint]` forbids inner whitespace;
            # strict tools read the number as a type, so accepting it as a bound would diverge.
            raise ValueError(f'spaced occurrence bound near {t!r}: write n*m with no whitespace (RFC 8610 occur)')
        return True

    def occurrence(self):
        t = self.peek()
        if t in ('?', '+', '*'):
            self.take()
            lo = int(t == '+')
            hi = 1 if t == '?' else None
            if t == '*' and (self.peek() or '').isdigit() and self.bound_digit(0):
                hi = int(self.take())
            return bounds(lo, hi)
        if (t or '').isdigit() and self.peek(1) == '*':
            if not self.adjacent(1):
                raise ValueError(f'spaced occurrence bound near {t!r}: write n*m with no whitespace (RFC 8610 occur)')
            lo = int(self.take())
            self.take('*')
            hi = int(self.take()) if (self.peek() or '').isdigit() and self.bound_digit(0) else None
            return bounds(lo, hi)
        return {'min': 1, 'max': 1}

    def group(self, end):
        alternatives = []
        while True:
            entries = []
            while self.peek() != end and self.peek() != '//':
                occ = self.occurrence()
                if self.peek(1) == ':':
                    key = self.take()
                    if key.startswith('"'):
                        key = json.loads(key)
                    elif not NAME.fullmatch(key):
                        raise ValueError(f'unsupported map key {key!r}')
                    self.take(':')
                    entry = {'kind': 'field', 'name': key, 'type': self.expr(), **occ}
                else:
                    value = self.expr()
                    if self.peek() == '=>':
                        self.take()
                        entry = {'kind': 'rest', 'key': value, 'type': self.expr(), **occ}
                    else:
                        entry = {'kind': 'value', 'type': value, **occ}
                entries.append(entry)
                if self.peek() != ',':
                    break
                self.take(',')
            alternatives.append(entries)
            if self.peek() != '//':
                break
            self.take('//')
        self.take(end)
        return {'op': 'group', 'alternatives': alternatives}

    def atom(self):
        t = self.take()
        if t in ('{', '['):
            return {'op': 'container', 'kind': t, 'group': self.group('}' if t == '{' else ']')}
        if t == '(':
            if self.peek() == ')':
                self.take(')')
                return {'op': 'group', 'alternatives': [[]]}
            start = self.i
            # A colon/comma/group choice at nesting depth zero disambiguates
            # a group from a parenthesized type expression.
            level = 0
            is_group = False
            for tok in self.tokens[start:]:
                if tok == ')' and level == 0:
                    break
                if level == 0 and tok in (':', ',', '//'):
                    is_group = True
                if tok in ('(', '{', '['): level += 1
                if tok in (')', '}', ']'): level -= 1
            if is_group:
                return self.group(')')
            node = self.expr()
            self.take(')')
            return node
        if re.fullmatch(r'-?[0-9]+', t):
            lo = int(t)
            if self.peek() == '..':
                self.take()
                hi = int(self.take())
                if hi < lo:
                    raise ValueError(f'reversed range {lo}..{hi}')
                return {'op': 'range', 'min': lo, 'max': hi}
            return {'op': 'literal', 'value': lo}
        if t.startswith('"') or t in ('true', 'false', 'null', 'nil'):
            return {'op': 'literal', 'value': None if t == 'nil' else json.loads(t)}
        if t in ('tstr', 'bool'):
            return {'op': 'primitive', 'name': t}
        if not NAME.fullmatch(t):
            raise ValueError(f'unsupported CDDL token {t!r}')
        return {'op': 'ref', 'name': t}

    def rules(self):
        rules = {}
        while self.peek() is not None:
            name = self.take()
            if not NAME.fullmatch(name):
                raise ValueError(f'invalid rule name {name!r}')
            assignment = self.take()
            if assignment not in ('=', '//=', '/='):
                raise ValueError(f'unsupported rule assignment {assignment!r}')
            node = self.expr()
            if assignment == '=':
                if name in rules:
                    raise ValueError(f'duplicate rule {name}')
                rules[name] = node
            else:
                if name not in rules:
                    # Group sockets are open groups, but not type sockets.
                    if not name.startswith('$$') or assignment != '//=':
                        raise ValueError(f'extension of undefined rule {name}')
                    rules[name] = {'op': 'group', 'alternatives': []}
                old = rules[name]
                if assignment == '//=':
                    if old['op'] != 'group' or node['op'] != 'group':
                        raise ValueError('unsupported group extension: expected groups')
                    old['alternatives'].extend(node['alternatives'])
                else:
                    rules[name] = choice([old, node])
        return rules


def lower(rules):
    def group_alts(group, stack=()):
        if group['op'] == 'ref':
            name = group['name']
            if name in stack:
                raise ValueError(f'recursive group {name}')
            if name not in rules:
                raise ValueError(f'undefined group {name}')
            return group_alts(rules[name], stack + (name,))
        if group['op'] != 'group':
            raise ValueError('unsupported group splice: expected group')
        result = []
        for alternative in group['alternatives']:
            variants = [[]]
            for entry in alternative:
                typ = entry['type']
                target = rules.get(typ.get('name')) if typ['op'] == 'ref' else typ
                if entry['kind'] == 'value' and target and target['op'] == 'group':
                    if entry['min'] != 1 or entry.get('max') != 1:
                        raise ValueError('unsupported repeated/optional group splice')
                    expanded = group_alts(typ, stack)
                    variants = [v + a for v in variants for a in expanded]
                else:
                    variants = [v + [entry] for v in variants]
                if len(variants) > 4096:
                    raise ValueError('group expansion limit')
            result.extend(variants)
        return result

    def visit(node):
        if node['op'] == 'container':
            variants = []
            for entries in group_alts(node['group']):
                if node['kind'] == '{':
                    fields, rest = [], None
                    for e in entries:
                        if e['kind'] == 'field':
                            if e['min'] not in (0, 1) or e.get('max') != 1:
                                raise ValueError('unsupported repeated map field')
                            if any(f['name'] == e['name'] for f in fields):
                                raise ValueError(f'duplicate field {e["name"]}')
                            fields.append({'name': e['name'], 'optional': e['min'] == 0, 'type': visit(e['type'])})
                        elif e['kind'] == 'rest' and rest is None:
                            rest = {'item': visit(e['type']), **{k: e[k] for k in ('min', 'max') if k in e}}
                            if e['key'] != {'op': 'primitive', 'name': 'tstr'}:
                                rest['key'] = visit(e['key'])
                        else:
                            raise ValueError('unsupported map entry/group')
                    if not fields and rest is not None:
                        variants.append({'op': 'map', **rest})
                    else:
                        variants.append({'op': 'record', 'fields': fields, **({} if rest is None else {'rest': rest})})
                else:
                    if any(e['kind'] != 'value' for e in entries):
                        raise ValueError('unsupported labeled tuple entry')
                    items = [{'type': visit(e['type']), **{k: e[k] for k in ('min', 'max') if k in e}} for e in entries]
                    if len(items) == 1 and items[0].get('max') != 1:
                        e = items[0]
                        variants.append({'op': 'array', 'item': e['type'], **{k: e[k] for k in ('min', 'max') if k in e}})
                    else:
                        variants.append({'op': 'tuple', 'items': items})
            return choice(variants)
        if node['op'] == 'group':
            # Groups have no standalone wire value; retain only container uses.
            return None
        return {k: visit(v) if isinstance(v, dict) else [visit(x) if isinstance(x, dict) else x for x in v] if isinstance(v, list) else v for k, v in node.items()}
    out = {name: visit(node) for name, node in rules.items() if node['op'] != 'group'}
    def refs(n):
        if isinstance(n, dict):
            if n.get('op') == 'ref' and n['name'] not in out:
                raise ValueError(f'undefined rule or group used as type: {n["name"]}')
            for v in n.values(): refs(v)
        elif isinstance(n, list):
            for v in n: refs(v)
    refs(out)
    return out


def parse(text):
    return lower(Parser(text).rules())


def parse_files(files):
    """Map file stem -> text; refs resolve local, then common, then unique.

    Always expose qualified rules; expose unqualified aliases only if unique.
    No concatenation-order dependency or silent duplicate-rule overwrite.
    """
    raw = {ns: Parser(text).rules() for ns, text in sorted(files.items())}
    merged = {}
    for ns, rules in raw.items():
        def qualify(n):
            if isinstance(n, dict):
                if n.get('op') == 'ref':
                    name = n['name']
                    candidates = [f for f, rs in raw.items() if name in rs]
                    target = ns if name in rules else 'common' if name in raw.get('common', {}) else candidates[0] if len(candidates) == 1 else None
                    if target is None:
                        raise ValueError(f'{ns}: ambiguous or undefined rule {name!r}: {candidates}')
                    n['name'] = target + '::' + name
                else:
                    for v in n.values(): qualify(v)
            elif isinstance(n, list):
                for v in n: qualify(v)
        for name, node in rules.items():
            node = copy.deepcopy(node)
            qualify(node)
            merged[ns + '::' + name] = node
    result = lower(merged)
    counts = {}
    for name in result:
        leaf = name.split('::')[1]
        counts[leaf] = counts.get(leaf, 0) + 1
    for name, node in list(result.items()):
        leaf = name.split('::')[1]
        if counts[leaf] == 1:
            result[leaf] = copy.deepcopy(node)
    return result
