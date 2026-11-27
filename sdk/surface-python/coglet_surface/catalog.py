"""Supervisor-only, exact-byte pinned extension catalogs (artifact version 1)."""
from dataclasses import dataclass
import hashlib
import json
import re

from .codec import encode_profile
from .generated import SCHEMA

COMPONENT = r'[a-z][a-z0-9_]{0,31}'
NAME01 = rf'{COMPONENT}\.{COMPONENT}'
NAME02 = rf'(?:{NAME01}|softmax\.app\.{COMPONENT})'


def profile(value):
    if type(value) is not str or value not in ('surface-0.1', 'surface-0.2'):
        raise ValueError('unknown contract profile')
    return value


def name(value, contract_profile):
    if type(value) is not str or not re.fullmatch(
            NAME02 if profile(contract_profile) == 'surface-0.2' else NAME01, value):
        raise ValueError('invalid capability name')


def wire_schema(version):
    # Select generated roots, never synthesize a widened copy of frozen IR.
    if version not in ('0.1', '0.2'):
        raise ValueError('unknown wire version')
    result = dict(SCHEMA)
    if version == '0.2':
        for family, rules in (
                ('common', ('capability-name', 'event', 'action')),
                ('surface', ('surface-message', 'cap-entry', 'capabilities',
                             'grant', 'grants', 'read', 'act', 'schema-refs'))):
            for rule in rules:
                target = f'surface02::s02-{rule}'
                if target not in SCHEMA:
                    raise ValueError(f'missing generated rule: {target}')
                result[f'{family}::{rule}'] = dict(op='ref', name=target)
    return result


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate catalog key')
        result[key] = value
    return result


def _keys(value, required, optional=()):
    if type(value) is not dict or not set(required) <= value.keys() or value.keys() - set(required) - set(optional):
        raise ValueError('unknown/missing catalog field')


def _portable_pattern(pattern):
    """Refuse regex syntax whose language differs from Rust's Unicode engine.

    Frozen validators are unchanged. Explicit classes and ordinary repetitions
    are portable; Unicode shorthand, flags, lookaround and backreferences are
    not. The exact universal counted-text form is safe in all three engines.
    """
    if '(?' in pattern.replace('(?:', '') or any(m in pattern for m in ('[:', '&&', '--', '~~', '||')):
        raise ValueError('nonportable regexp')
    try:
        re.compile(pattern, re.ASCII)
    except re.error as exc:
        raise ValueError('invalid regexp') from exc
    if re.fullmatch(r'\^\[\\s\\S\]\{[0-9]+,[0-9]+\}\$', pattern):
        return
    in_class = False
    i = 0
    while i < len(pattern):
        c = pattern[i]
        if c == '\\':
            i += 1
            if i == len(pattern):
                raise ValueError('truncated regexp escape')
            c = pattern[i]
            if c.isalnum() and c not in 'nrtfx':
                raise ValueError('nonportable regexp escape')
            if c == 'x':
                if not re.fullmatch('[0-9a-fA-F]{2}', pattern[i+1:i+3]):
                    raise ValueError('nonportable hex escape')
                i += 2
        elif c == '[':
            if in_class:
                raise ValueError('nested regexp class')
            in_class = True
        elif c == ']':
            in_class = False
        elif c in '+*?}' and pattern[i+1:i+2] == '+':
            raise ValueError('nonportable possessive repetition')
        i += 1


def _ir(node, rules, stack=(), depth=0):
    if depth > 64 or type(node) is not dict:
        raise ValueError('invalid/nested schema IR')
    op = node.get('op')
    keys = {'ref': ('name',), 'record': ('fields',), 'union': ('items',),
            'intersection': ('items',), 'array': ('min', 'max', 'item'),
            'primitive': ('name',), 'range': ('min', 'max'), 'size': ('min', 'max'),
            'literal': ('value',), 'regexp': ('pattern',)}
    if type(op) is not str or op not in keys:
        raise ValueError('unsupported/open schema IR')
    _keys(node, ('op', *keys[op]))
    def child(n, trail=stack):
        _ir(n, rules, trail, depth + 1)
    if op == 'ref':
        ref = node['name']
        if type(ref) is not str or ref not in rules or ref in stack:
            raise ValueError('unknown/recursive IR reference')
        child(rules[ref], (*stack, ref))
    elif op == 'record':
        if type(node['fields']) is not list:
            raise ValueError('invalid fields')
        seen = set()
        for field in node['fields']:
            _keys(field, ('name', 'optional', 'type'))
            if type(field['name']) is not str or field['name'] in seen or type(field['optional']) is not bool:
                raise ValueError('invalid/duplicate field')
            seen.add(field['name'])
            child(field['type'])
    elif op in ('union', 'intersection'):
        if type(node['items']) is not list or not node['items']:
            raise ValueError('empty IR alternatives')
        for item in node['items']:
            child(item)
    elif op in ('range', 'size', 'array'):
        lo, hi = node['min'], node['max']
        if (type(lo) is not int or type(hi) is not int or not -(2**63) <= lo <= hi < 2**64
                or (op != 'range' and lo < 0) or (op == 'array' and hi > 262144)):
            raise ValueError('invalid IR bounds')
        if op == 'array':
            child(node['item'])
    elif op == 'primitive':
        if node['name'] not in ('tstr', 'bool', 'null'):
            raise ValueError('invalid primitive')
    elif op == 'regexp':
        if type(node['pattern']) is not str:
            raise ValueError('invalid regexp')
        _portable_pattern(node['pattern'])
    elif op == 'literal':
        encode_profile(node['value'])


@dataclass(frozen=True, init=False)
class TrustedExtensionCatalog:
    raw: bytes
    sha256: str
    contract_profile: str

    def __init__(self, raw, sha256, *, contract_profile='surface-0.1'):
        profile(contract_profile)
        if (type(raw) is not bytes or len(raw) > 1_048_576 or type(sha256) is not str
                or not re.fullmatch('[0-9a-f]{64}', sha256) or hashlib.sha256(raw).hexdigest() != sha256):
            raise ValueError('trusted catalog pin mismatch or byte limit')
        artifact = json.loads(raw.decode('utf-8'), object_pairs_hook=_object,
                              parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))
        # Enforce Unicode scalar values and the common integer/value profile too.
        encode_profile(artifact)
        _keys(artifact, ('version', 'namespace', 'rules', 'schemas', 'entries', 'operations'))
        ns = artifact['namespace']
        if (type(artifact['version']) is not int or artifact['version'] != 1
                or type(ns) is not str or not re.fullmatch(COMPONENT, ns)
                or ns in ('runtime', 'speech', 'expression', 'llm', 'action', 'hub', 'test', 'sim', 'sensor', 'motor', 'other')):
            raise ValueError('unsupported namespace/version')
        for field in ('rules', 'schemas', 'entries', 'operations'):
            if type(artifact[field]) is not dict or not artifact[field]:
                raise ValueError('empty/invalid catalog section')
        def local(n, app=True):
            name(n, contract_profile if app else 'surface-0.1')
            if not n.startswith(ns + '.'):
                raise ValueError('name outside namespace')
        for entry, resources in artifact['entries'].items():
            local(entry, False)
            if type(resources) is not list or len(resources) != len(set(resources)):
                raise ValueError('invalid entry resources')
            for resource in resources:
                local(resource, False)
        rules = artifact['rules']
        for key, node in rules.items():
            _ir(node, rules, (key,))
        for uri, rule in artifact['schemas'].items():
            if (not uri.startswith('https://') or 'coglet.dev/spec/messages/' in uri
                    or any(c.isspace() for c in uri) or type(rule) is not str or rule not in rules):
                raise ValueError('invalid schema URI/reference')
        for operation, binding in artifact['operations'].items():
            local(operation)
            _keys(binding, ('entry', 'direction', 'input', 'resources', 'single_owner'), ('output', 'selection'))
            direction = binding['direction']
            if (direction not in ('event', 'read', 'action') or binding['single_owner'] is not True
                    or binding['entry'] not in artifact['entries'] or binding['input'] not in artifact['schemas']
                    or (direction == 'event' and binding.get('output') is not None)
                    or (direction != 'event' and binding.get('output') not in artifact['schemas'])):
                raise ValueError('invalid operation binding')
            resources = binding['resources']
            if (type(resources) is not list or len(resources) != len(set(resources))
                    or not set(resources) <= set(artifact['entries'][binding['entry']])):
                raise ValueError('invalid operation resources')
            selection = binding.get('selection')
            if selection is not None:
                _keys(selection, ('operation_field', 'args_field', 'value_field', 'reads'))
                fields = [selection[k] for k in ('operation_field', 'args_field', 'value_field')]
                reads = selection['reads']
                if (direction != 'event' or any(type(f) is not str or not f for f in fields)
                        or len(set(fields)) != 3 or type(reads) is not list or not reads
                        or len(reads) != len(set(reads))
                        or any(artifact['operations'].get(r, {}).get('direction') != 'read' for r in reads)):
                    raise ValueError('invalid selected-read binding')
            if operation == 'game.observation' and selection is None:
                raise ValueError('missing selected-read binding')
        object.__setattr__(self, 'raw', raw)
        object.__setattr__(self, 'sha256', sha256)
        object.__setattr__(self, 'contract_profile', contract_profile)

    @property
    def artifact(self):
        """Return a detached copy; retained authority is immutable verified bytes."""
        return json.loads(self.raw)

    @classmethod
    def from_pinned_bytes(cls, raw, sha256, *, contract_profile='surface-0.1'):
        return cls(raw, sha256, contract_profile=contract_profile)
