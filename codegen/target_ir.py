"""Fail-closed additive IR v2 native emitter boundary.

The historical require_v1 entry point is retained for import compatibility;
all native codecs now enforce these additive constraints before encoding.
"""
NODE_KEYS = {
    'ref': ({'op', 'name'}, set()),
    'literal': ({'op', 'value'}, set()),
    'primitive': ({'op', 'name'}, set()),
    'range': ({'op', 'min', 'max'}, set()),
    'union': ({'op', 'items'}, set()),
    'intersection': ({'op', 'items'}, set()),
    'default': ({'op', 'item', 'value'}, set()),
    'tuple': ({'op', 'items'}, set()),
    'regexp': ({'op', 'pattern'}, set()),
    'size': ({'op', 'min', 'max'}, set()),
    'record': ({'op', 'fields'}, {'rest'}),
    'array': ({'op', 'item', 'min'}, {'max'}),
    'map': ({'op', 'item'}, {'min', 'max', 'key'}),
}


def require_v1(rules):
    def keys(node, required, optional=()):
        if not isinstance(node, dict) or not required <= node.keys() or node.keys() - required - set(optional):
            raise ValueError(f'unsupported native target IR operation/constraints: {node!r}')

    def occurrence(node):
        for k in ('min', 'max'):
            if k in node and (type(node[k]) is not int or node[k] < 0):
                raise ValueError(f'invalid occurrence {node}')
        if 'max' in node and node['max'] < node.get('min', 0):
            raise ValueError(f'reversed occurrence {node}')

    def walk(node):
        op = node.get('op') if isinstance(node, dict) else None
        if op not in NODE_KEYS:
            raise ValueError(f'unsupported native target IR operation/constraints: {op}')
        keys(node, *NODE_KEYS[op])
        if op == 'ref':
            if node['name'] not in rules:
                raise ValueError(f'undefined native IR reference {node["name"]}')
        elif op == 'primitive':
            if node['name'] not in ('tstr', 'bool', 'null'):
                raise ValueError(f'unknown primitive {node["name"]}')
        elif op in ('union', 'intersection'):
            if not node['items']:
                raise ValueError(f'empty {op}')
            for child in node['items']: walk(child)
        elif op == 'default':
            walk(node['item'])
        elif op == 'tuple':
            for entry in node['items']:
                keys(entry, {'type', 'min'}, {'max'})
                occurrence(entry)
                walk(entry['type'])
        elif op == 'record':
            for entry in node['fields']:
                keys(entry, {'name', 'optional', 'type'})
                walk(entry['type'])
            if 'rest' in node:
                rest = node['rest']
                keys(rest, {'item', 'min'}, {'max', 'key'})
                occurrence(rest)
                walk(rest['item'])
                if 'key' in rest: walk(rest['key'])
        elif op in ('array', 'map'):
            occurrence(node)
            walk(node['item'])
            if 'key' in node: walk(node['key'])
    for node in rules.values(): walk(node)
