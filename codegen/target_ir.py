"""Fail closed at native emitter boundaries when the compiler IR evolves.

Go/Java currently implement the v1 IR. Reject additive constraints as well as
new operations: ignoring an unfamiliar max/key/rest would broaden acceptance.
The compiler owns IR evolution; expand this contract only alongside codecs.
"""

NODE_KEYS = {
    'ref': {'op', 'name'},
    'literal': {'op', 'value'},
    'primitive': {'op', 'name'},
    'range': {'op', 'min', 'max'},
    'union': {'op', 'items'},
    'regexp': {'op', 'pattern'},
    'size': {'op', 'min', 'max'},
    'record': {'op', 'fields'},
    'array': {'op', 'item', 'min'},
    'map': {'op', 'item'},
}


def require_v1(rules):
    def walk(value):
        if isinstance(value, list):
            for child in value:
                walk(child)
        elif isinstance(value, dict):
            if 'op' in value:
                op = value['op']
                if op not in NODE_KEYS or set(value) != NODE_KEYS[op]:
                    raise ValueError(f'unsupported native target IR operation/constraints: {op}: {sorted(value)}')
                # Literal metadata is data, not an expression subtree.
                if op == 'literal':
                    return
            for child in value.values():
                walk(child)
    for node in rules.values():
        walk(node)
