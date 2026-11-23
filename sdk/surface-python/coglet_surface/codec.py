"""Shared strict deterministic CBOR codec; shapes come only from generated CDDL IR."""
from dataclasses import fields, is_dataclass
import io
import re
import types
from typing import get_args, get_origin, get_type_hints, ClassVar

import cbor2

MAX_DEPTH = 64


def encode_profile(value, _depth=0):
    if _depth > MAX_DEPTH:
        raise ValueError('CBOR nesting limit')
    if isinstance(value, dict):
        if not all(type(key) is str for key in value):
            raise ValueError('map keys must be text')
        pairs = sorted((encode_profile(k, _depth + 1), encode_profile(v, _depth + 1))
                       for k, v in value.items())
        return _header(5, len(pairs)) + b''.join(k + v for k, v in pairs)
    if isinstance(value, list):
        return _header(4, len(value)) + b''.join(encode_profile(v, _depth + 1) for v in value)
    if value is None or type(value) in (bool, str):
        return cbor2.dumps(value)
    if type(value) is int and -(2**63) <= value < 2**64:
        return cbor2.dumps(value)
    raise ValueError('value outside CBOR profile')


def _header(major, size):
    raw = cbor2.dumps(size)
    return bytes([(major << 5) | (raw[0] & 31)]) + raw[1:]


def decode_profile(raw):
    # Scan first: cbor2 normalizes tags (e.g. tag 55799) and simple 20/21/22,
    # losing evidence needed to reject forbidden wire forms.
    pos = 0

    def scan(depth):
        nonlocal pos
        if depth > MAX_DEPTH or pos >= len(raw):
            raise ValueError('CBOR nesting limit or truncated item')
        head = raw[pos]
        pos += 1
        major, ai = head >> 5, head & 31
        if major not in (0, 1, 3, 4, 5, 7) or ai >= 28:
            raise ValueError('forbidden CBOR type or indefinite length')
        if major == 7:
            if ai not in (20, 21, 22):
                raise ValueError('forbidden CBOR simple/float')
            return
        size = ai
        if ai >= 24:
            width = 1 << (ai - 24)
            if pos + width > len(raw):
                raise ValueError('truncated CBOR head')
            size = int.from_bytes(raw[pos:pos + width], 'big')
            pos += width
            if size < (24 if width == 1 else 1 << (8 * (width // 2))):
                raise ValueError('non-shortest CBOR head')
        if major == 1 and size >= 2**63:
            raise ValueError('negative integer outside profile')
        if major == 3:
            if pos + size > len(raw):
                raise ValueError('truncated text')
            raw[pos:pos + size].decode('utf-8')
            pos += size
        if major in (4, 5):
            count = size * (2 if major == 5 else 1)
            if count > len(raw) - pos:
                raise ValueError('truncated container')
            previous = None
            for i in range(count):
                start = pos
                if major == 5 and i % 2 == 0 and raw[pos] >> 5 != 3:
                    raise ValueError('nontext map key')
                scan(depth + 1)
                if major == 5 and i % 2 == 0:
                    key = raw[start:pos]
                    if previous is not None and key <= previous:
                        raise ValueError('duplicate or unsorted map key')
                    previous = key
    try:
        scan(0)
        if pos != len(raw):
            raise ValueError('trailing CBOR data')
        value = cbor2.CBORDecoder(io.BytesIO(raw)).decode()
        if encode_profile(value) != raw:
            raise ValueError('non-deterministic CBOR item')
        return value
    except (cbor2.CBORDecodeError, UnicodeError, RecursionError, IndexError) as exc:
        raise ValueError('invalid CBOR') from exc


def validate(rule, value):
    from .generated import SCHEMA

    def check(node, val, depth=0):
        if depth > 128:
            raise ValueError('schema recursion limit')
        op = node['op']
        if op == 'ref':
            if node['name'] not in SCHEMA:
                raise ValueError('unknown rule ' + node['name'])
            return check(SCHEMA[node['name']], val, depth + 1)
        if op == 'union':
            for branch in node['items']:
                try:
                    check(branch, val, depth + 1)
                    return
                except ValueError:
                    pass
            raise ValueError('no union branch matched')
        good = False
        if op == 'literal':
            good = type(val) is type(node['value']) and val == node['value']
        elif op == 'primitive':
            good = type(val) is {'tstr': str, 'bool': bool, 'null': type(None)}[node['name']]
        elif op == 'range':
            good = type(val) is int and node['min'] <= val <= node['max']
        elif op == 'regexp':
            good = type(val) is str and re.fullmatch(node['pattern'], val, flags=re.ASCII) is not None
        elif op == 'size':
            good = type(val) is str and node['min'] <= len(val.encode('utf-8')) <= node['max']
        elif op == 'array':
            good = type(val) is list and len(val) >= node['min']
            if good:
                for item in val:
                    check(node['item'], item, depth + 1)
        elif op == 'map':
            good = type(val) is dict and all(type(k) is str for k in val)
            if good:
                for item in val.values():
                    check(node['item'], item, depth + 1)
        elif op == 'record':
            good = type(val) is dict
            if good:
                allowed = {field['name'] for field in node['fields']}
                good = not val.keys() - allowed
                for field in node['fields']:
                    if field['name'] in val:
                        check(field['type'], val[field['name']], depth + 1)
                    elif not field['optional']:
                        good = False
        if not good:
            raise ValueError(f'value does not match {op}')
    if rule not in SCHEMA:
        raise ValueError('unknown rule ' + rule)
    # This also bounds recursive structures and rejects non-profile payloads.
    encode_profile(value)
    check(SCHEMA[rule], value)


def _plain(value):
    if is_dataclass(value):
        # Only absent optional fields are omitted; required null remains present.
        return {f.name: _plain(getattr(value, f.name)) for f in fields(value)
                if getattr(value, f.name) is not None or f.default is not None}
    if isinstance(value, list):
        return [_plain(v) for v in value]
    if isinstance(value, dict):
        return {k: _plain(v) for k, v in value.items()}
    return value


def encode(rule, value):
    value = _plain(value)
    validate(rule, value)
    return encode_profile(value)


def decode(rule, raw):
    value = decode_profile(raw)
    validate(rule, value)
    return value


def _construct(annotation, value):
    if isinstance(annotation, type) and issubclass(annotation, Message):
        hints = get_type_hints(annotation)
        return annotation(**{k: _construct(hints[k], v) for k, v in value.items()})
    if get_origin(annotation) is list:
        return [_construct(get_args(annotation)[0], v) for v in value]
    if get_origin(annotation) is types.UnionType:
        for branch in get_args(annotation):
            if isinstance(branch, type) and issubclass(branch, Message) and isinstance(value, dict):
                try:
                    return _construct(branch, value)
                except (KeyError, TypeError):
                    pass
    return value


class Message:
    _rule: ClassVar[str]

    def to_cbor(self):
        return encode(self._rule, self)

    @classmethod
    def from_cbor(cls, raw):
        value = decode(cls._rule, raw)
        result = _construct(cls, value)
        # A class representing one union branch must not accept another branch.
        if result.to_cbor() != raw:
            raise ValueError('wrong record variant')
        return result
