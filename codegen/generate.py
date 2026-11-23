#!/usr/bin/env python3
"""Fail-closed CDDL subset compiler. No third-party generation dependencies."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TOKEN = re.compile(r'\s+|;[^\n]*|"(?:[^"\\]|\\.)*"|-?[0-9]+|[A-Za-z_][A-Za-z0-9_-]*|\.regexp|\.size|\.\.|=>|[{}\[\](),:?+*/=]')


def parse(text):
    tokens = []
    pos = 0
    while pos < len(text):
        m = TOKEN.match(text, pos)
        if not m:
            raise ValueError(f'unsupported CDDL at {text[pos:pos+40]!r}')
        token = m.group()
        if not token.isspace() and not token.startswith(';'):
            tokens.append(token)
        pos = m.end()
    i = 0

    def take(expected=None):
        nonlocal i
        if i == len(tokens):
            raise ValueError('unexpected end of CDDL')
        value = tokens[i]
        if expected is not None and value != expected:
            raise ValueError(f'expected {expected!r}, got {value!r}')
        i += 1
        return value

    def peek():
        return tokens[i] if i < len(tokens) else None

    def expr():
        items = [atom()]
        while peek() == '/':
            take('/')
            items.append(atom())
        return items[0] if len(items) == 1 else {'op': 'union', 'items': items}

    def atom():
        token = take()
        if token == '(':
            result = expr()
            take(')')
            return result
        if token == '{':
            if peek() == '*':
                take('*')
                take('tstr')
                take('=>')
                result = {'op': 'map', 'item': expr()}
                take('}')
                return result
            fields = []
            while peek() != '}':
                optional = peek() == '?'
                if optional:
                    take('?')
                name = take()
                if name.startswith('"'):
                    name = json.loads(name)
                take(':')
                fields.append({'name': name, 'optional': optional, 'type': expr()})
                if peek() != ',':
                    break
                take(',')
            take('}')
            return {'op': 'record', 'fields': fields}
        if token == '[':
            count = take()
            if count not in ('*', '+'):
                raise ValueError('only homogeneous * and + arrays are supported')
            result = {'op': 'array', 'min': int(count == '+'), 'item': expr()}
            take(']')
            return result
        if re.fullmatch(r'-?[0-9]+', token):
            low = int(token)
            if peek() == '..':
                take('..')
                return {'op': 'range', 'min': low, 'max': int(take())}
            return {'op': 'literal', 'value': low}
        if token.startswith('"') or token in ('true', 'false', 'null'):
            return {'op': 'literal', 'value': json.loads(token)}
        if token == 'tstr' and peek() in ('.regexp', '.size'):
            control = take()
            if control == '.regexp':
                return {'op': 'regexp', 'pattern': json.loads(take())}
            take('(')
            low = int(take())
            take('..')
            high = int(take())
            take(')')
            return {'op': 'size', 'min': low, 'max': high}
        if token in ('tstr', 'bool'):
            return {'op': 'primitive', 'name': token}
        if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', token):
            raise ValueError(f'unsupported CDDL token {token}')
        return {'op': 'ref', 'name': token}

    rules = {}
    while i < len(tokens):
        name = take()
        take('=')
        if name in rules:
            raise ValueError(f'duplicate rule {name}')
        rules[name] = expr()
    def refs(node):
        if isinstance(node, dict):
            if node.get('op') == 'ref' and node['name'] not in rules:
                raise ValueError(f'undefined rule {node["name"]}')
            for child in node.values():
                refs(child)
        elif isinstance(node, list):
            for child in node:
                refs(child)
    refs(rules)
    return rules


def title(name):
    return ''.join(part.title() for part in name.replace('_', '-').split('-'))


def rust_type(node):
    op = node['op']
    if op == 'ref':
        return title(node['name'])
    if op in ('regexp', 'size') or op == 'primitive' and node['name'] == 'tstr':
        return 'String'
    if op == 'primitive' and node['name'] == 'bool':
        return 'bool'
    if op == 'range' and node['min'] >= 0:
        return 'u64'
    if op == 'array':
        return f'Vec<{rust_type(node["item"])}>'
    if op == 'map':
        return 'std::collections::BTreeMap<String, ciborium::Value>'
    if op == 'literal' and type(node['value']) is bool:
        return 'bool'
    if op == 'literal' and isinstance(node['value'], str):
        return 'String'
    if op == 'union' and all(n['op'] in ('literal', 'ref') for n in node['items']):
        # Text-only aliases are ergonomic; mixed/recursive values use CBOR Value.
        if all(n['op'] == 'literal' and isinstance(n['value'], str) for n in node['items']):
            return 'String'
        if all(n['op'] == 'ref' and n['name'].endswith('-id') for n in node['items']):
            return 'String'
    return 'ciborium::Value'


def python_type(node):
    op = node['op']
    if op == 'ref':
        return title(node['name'])
    if op in ('regexp', 'size') or op == 'primitive' and node['name'] == 'tstr':
        return 'str'
    if op == 'primitive' and node['name'] == 'bool':
        return 'bool'
    if op == 'range':
        return 'int'
    if op == 'literal':
        return 'Literal[' + repr(node['value']) + ']'
    if op == 'array':
        return f'list[{python_type(node["item"])}]'
    if op == 'map':
        return 'dict[str, Any]'
    return 'Any'


def generate(rules):
    from swift import generate_swift
    swift = generate_swift(rules)
    ir = json.dumps(rules, sort_keys=True, separators=(',', ':'))
    rust = ['// Generated by codegen/run. DO NOT EDIT.', 'use serde::{Deserialize, Serialize};',
            'pub const SCHEMA_JSON: &str = r###"' + ir + '"###;']
    py = ['# Generated by codegen/run. DO NOT EDIT.', 'from __future__ import annotations',
          'from dataclasses import dataclass', 'from typing import Any, ClassVar, Literal',
          'import json', 'from .codec import Message', 'SCHEMA = json.loads(' + repr(ir) + ')']
    registry = {}
    for name, node in rules.items():
        variants = node['items'] if node['op'] == 'union' else [node]
        if all(v['op'] == 'record' for v in variants):
            names = []
            for index, variant in enumerate(variants):
                cls = title(name) + (str(index + 1) if len(variants) > 1 else '')
                names.append(cls)
                rust += ['#[derive(Debug, Clone, Serialize, Deserialize)]', '#[serde(deny_unknown_fields)]', f'pub struct {cls} {{']
                py += ['@dataclass', f'class {cls}(Message):', f'    _rule: ClassVar[str] = {name!r}']
                # Required fields first: Python dataclasses prohibit defaults before required.
                fields = sorted(variant['fields'], key=lambda f: f['optional'])
                for field in fields:
                    rt = rust_type(field['type'])
                    pt = python_type(field['type'])
                    key = field['name']
                    ident = key.replace('-', '_')
                    if key in ('type', 'ref', 'match', 'self'):
                        ident += '_'
                    if ident != key:
                        rust.append(f'#[serde(rename = {json.dumps(key)})]')
                    if field['optional']:
                        rust.append('#[serde(default, skip_serializing_if = "Option::is_none")]')
                        rt = f'Option<{rt}>'
                        pt += ' | None = None'
                    rust.append(f'pub {ident}: {rt},')
                    py.append(f'    {ident}: {pt}')
                rust.append('}')
                rust += [f'impl {cls} {{', 'pub fn to_cbor(&self) -> Result<Vec<u8>, String> {', f'crate::encode_typed({json.dumps(name)}, self)', '}', 'pub fn from_cbor(raw: &[u8]) -> Result<Self, String> {', f'crate::decode_typed({json.dumps(name)}, raw)', '}', '}']
            registry[name] = names
            if len(names) > 1:
                rust += ['#[derive(Debug, Clone, Serialize, Deserialize)]', '#[serde(untagged)]', f'pub enum {title(name)} {{']
                rust += [f'Variant{i+1}({cls}),' for i, cls in enumerate(names)] + ['}']
                py.append(f'{title(name)} = ' + ' | '.join(names))
        else:
            rust.append(f'pub type {title(name)} = {rust_type(node)};')
            # Runtime aliases avoid unresolved or recursive assignments; field annotations
            # retain the CDDL names and all constraints are enforced by the IR validator.
            py.append(f'{title(name)} = {python_type(node) if node["op"] not in ("ref", "array") else "Any"}')
    py += ['RECORDS = {' + ', '.join(repr(k) + ': [' + ', '.join(v) + ']' for k, v in registry.items()) + '}']
    outputs = {'crates/coglet-types/src/generated.rs': '\n'.join(rust) + '\n',
               'sdk/surface-python/coglet_surface/generated.py': '\n'.join(py) + '\n'}
    formatted = subprocess.run(['rustfmt', '--edition', '2024'], input=outputs['crates/coglet-types/src/generated.rs'], text=True, capture_output=True, check=True)
    outputs['crates/coglet-types/src/generated.rs'] = formatted.stdout
    outputs['sdk/surface-swift/Sources/CogletTypes/Generated/Types.swift'] = swift
    return outputs


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    texts = [p.read_text() for p in sorted((ROOT / 'spec/messages').glob('*.cddl'))]
    rules = parse('\n'.join(texts))
    stale = []
    for path, content in generate(rules).items():
        path = ROOT / path
        if not path.exists() or path.read_text() != content:
            stale.append(str(path.relative_to(ROOT)))
            if not args.check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
    if args.check and stale:
        sys.exit('stale generated files: ' + ', '.join(stale))
    print(f'codegen: {len(rules)} CDDL rules; Rust/Python/Swift outputs ' + ('current' if args.check else 'written'))


if __name__ == '__main__':
    main()
