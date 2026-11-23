"""Generate Java 21 records and sealed record unions from the shared CDDL IR (version 2)."""
import json
import re
from target_ir import require_v1

KEYWORDS = set(('abstract assert boolean break byte case catch char class const continue default do double else enum extends final finally float for goto if implements import instanceof int interface long native new package private protected public return short static strictfp super switch synchronized this throw throws transient try void volatile while true false null record sealed permits yield var _ clone finalize getClass hashCode notify notifyAll toString wait').split())

# node op -> (required keys, optional keys). The emitter validates the IR itself so an
# unfamiliar operation or constraint can never be silently ignored (no weakening).
SHAPES = {
    'ref': ({'name'}, set()), 'literal': ({'value'}, set()), 'primitive': ({'name'}, set()),
    'range': ({'min', 'max'}, set()), 'union': ({'items'}, set()), 'regexp': ({'pattern'}, set()),
    'size': ({'min'}, {'max'}), 'record': ({'fields'}, {'rest'}), 'array': ({'item', 'min'}, {'max'}),
    'map': ({'item'}, {'min', 'max', 'key'}), 'intersection': ({'items'}, set()),
    'default': ({'item', 'value'}, set()), 'tuple': ({'items'}, set()),
}
PRIMITIVES = {'tstr', 'bool', 'null'}


def title(name):
    return ''.join(p[:1].upper() + p[1:] for p in re.split('::|[-_]', name))


def ident(name):
    result = re.sub('[^A-Za-z0-9_$]', '_', name)
    if not result or result[0].isdigit(): result = '_' + result
    if result in KEYWORDS: result += '_'
    return result


def _occurrence(where, e):
    if type(e['min']) is not int or e['min'] < 0 or 'max' in e and (type(e['max']) is not int or e['max'] < e['min']):
        raise ValueError(f'invalid occurrence bounds at {where}')


def check_ir(rules):
    def keys(where, value, required, optional):
        if not isinstance(value, dict) or not required <= set(value) <= required | optional:
            raise ValueError(f'unsupported Java IR shape at {where}: {sorted(value) if isinstance(value, dict) else value!r}')

    def node(n, where):
        if not isinstance(n, dict) or n.get('op') not in SHAPES:
            raise ValueError(f'unsupported Java IR operation at {where}: {n.get("op") if isinstance(n, dict) else n!r}')
        op = n['op']
        keys(where, n, SHAPES[op][0] | {'op'}, SHAPES[op][1])
        if op == 'ref' and n['name'] not in rules: raise ValueError(f'unknown reference {n["name"]!r} at {where}')
        if op == 'primitive' and n['name'] not in PRIMITIVES: raise ValueError(f'unsupported primitive {n["name"]!r} at {where}')
        if op == 'literal' and not (n['value'] is None or type(n['value']) in (str, bool, int)): raise ValueError(f'unsupported literal at {where}')
        for k in ('min', 'max'):
            if k in n and (type(n[k]) is not int or n[k] < 0 and op != 'range'): raise ValueError(f'invalid bound {k} at {where}')
        if 'max' in n and 'min' in n and n['max'] < n['min']: raise ValueError(f'empty bounds at {where}')
        if op in ('union', 'intersection'):
            if not isinstance(n['items'], list) or not n['items']: raise ValueError(f'empty {op} at {where}')
            for i in n['items']: node(i, where)
        elif op in ('array', 'default'): node(n['item'], where)
        elif op == 'map':
            node(n['item'], where)
            if 'key' in n: node(n['key'], where)
        elif op == 'tuple':
            if not isinstance(n['items'], list): raise ValueError(f'invalid tuple at {where}')
            for e in n['items']:
                keys(where, e, {'type', 'min'}, {'max'})
                _occurrence(where, e)
                node(e['type'], where)
        elif op == 'record':
            for f in n['fields']:
                keys(where, f, {'name', 'optional', 'type'}, set())
                node(f['type'], where)
            if 'rest' in n:
                r = n['rest']
                keys(where, r, {'item', 'min'}, {'max', 'key'})
                _occurrence(where, r)
                node(r['item'], where)
                if 'key' in r: node(r['key'], where)
    for name, n in rules.items(): node(n, name)


def aliases(rules):
    """Map `stem::rule` to its identical unqualified copy so one Java type serves both keys."""
    result = {}
    for k, v in rules.items():
        if '::' in k:
            base = k.split('::', 1)[1]
            if base in rules and rules[base] == v: result[k] = base
    return result


def generate(rules):
    require_v1(rules)
    check_ir(rules)
    alias = aliases(rules)
    canon = lambda n: alias.get(n, n)
    owned = {n: node for n, node in rules.items() if n not in alias}
    names = [title(n) for n in owned]
    if len(set(names)) != len(names) or any(n in {'Generated','Codec','Message','Presence','Object','String','Boolean','BigInteger','List','Map','LinkedHashMap'} for n in names):
        raise ValueError('ambiguous Java rule names')
    def variants(node):
        vs = node['items'] if node['op'] == 'union' else [node]
        return vs if all(v['op'] == 'record' for v in vs) else []
    def typ(node, seen=()):
        op = node['op']
        if op == 'ref':
            n = canon(node['name'])
            if variants(rules[n]): return title(n)
            return 'Object' if n in seen else typ(rules[n], seen+(n,))
        if op in ('regexp','size'): return 'String'
        if op == 'primitive': return {'tstr':'String','bool':'Boolean','null':'Object'}[node['name']]
        if op == 'range': return 'BigInteger'
        if op == 'literal':
            v=node['value']
            return 'String' if isinstance(v,str) else 'Boolean' if type(v) is bool else 'BigInteger' if type(v) is int else 'Object'
        if op == 'tuple': return 'List<Object>'
        if op == 'default': return typ(node['item'],seen)
        if op == 'intersection':
            types={typ(v,seen) for v in node['items']}
            return types.pop() if len(types)==1 else 'Object'
        if op == 'array': return 'List<' + typ(node['item'],seen) + '>'
        if op == 'map': return 'Map<String, ' + typ(node['item'],seen) + '>'
        if op == 'union':
            types={typ(v,seen) for v in node['items']}
            if len(types)==1: return types.pop()
        return 'Object'
    declared = ['Generated', 'Message', 'Presence']
    for name, node in owned.items():
        vs = variants(node)
        if vs:
            declared.append(title(name))
            if len(vs) > 1:
                declared.extend(title(name) + str(i + 1) for i in range(len(vs)))
    if len(set(declared)) != len(declared) or any(not re.fullmatch(r'[A-Za-z_$][A-Za-z0-9_$]*', n) for n in names):
        raise ValueError('ambiguous or invalid Java type names')
    out=['// Generated by codegen/run. DO NOT EDIT.', 'package org.coglet.surface;', 'import java.math.BigInteger;', 'import java.util.*;', 'public final class Generated {', 'private Generated() {}', 'public interface Message { String rule(); Object toValue(); default byte[] toCbor() { return Codec.encode(rule(), this); } }', 'public record Presence<T>(T value) {}']
    registrations=[]
    for name,node in owned.items():
        vs=variants(node)
        if not vs: continue
        base=title(name)
        classes=[base+(str(i+1) if len(vs)>1 else '') for i in range(len(vs))]
        if len(vs)>1:
            out += [f'public sealed interface {base} extends Message permits '+', '.join(classes)+' {', f'static {base} fromCbor(byte[] raw) {{ return ({base}) Codec.decodeTyped({json.dumps(name)}, raw); }}', '}']
        for cls,v in zip(classes,vs):
            fs=v['fields']; ids=[ident(f['name']) for f in fs]; rest=v.get('rest')
            if len(set(ids + (['rest$'] if rest else []))) != len(ids) + bool(rest) or any(i in {'rule','toValue','toCbor','fromCbor'} for i in ids): raise ValueError('ambiguous Java field names')
            args=[]
            for f,i in zip(fs,ids):
                t=typ(f['type'])
                if f['optional']: t=f'Presence<{t}>'
                args.append(t+' '+i)
            if rest: args.append('Map<String, ' + typ(rest['item']) + '> rest$')
            out += [f'public record {cls}('+', '.join(args)+f') implements {base if len(vs)>1 else "Message"} {{', f'public String rule() {{ return {json.dumps(name)}; }}', 'public Object toValue() { Map<String,Object> m = new LinkedHashMap<>();']
            for f,i in zip(fs,ids):
                val=f'{i}.value()' if f['optional'] else i
                prefix=f'if ({i} != null) ' if f['optional'] else ''
                out += [prefix+f'm.put({json.dumps(f["name"])}, {val});']
            if rest: out += ['if (rest$ != null) for (var e : rest$.entrySet()) { if (m.containsKey(e.getKey())) throw new IllegalArgumentException("rest key collides with field"); m.put(e.getKey(), e.getValue()); }']
            out += ['return m; }', f'public static {cls} fromCbor(byte[] raw) {{ return Codec.decodeRecord({json.dumps(name)}, raw, {cls}.class); }}', '}']
            registrations.append(f'Codec.register({json.dumps(name)}, {cls}.class, {json.dumps(json.dumps(v,separators=(",",":")))}, new String[]{{'+','.join(json.dumps(f['name']) for f in fs)+'});')
    registrations += [f'Codec.alias({json.dumps(a)}, {json.dumps(c)});' for a, c in sorted(alias.items()) if variants(rules[c])]
    out += ['static void registerAll() {']+registrations+['}', '}']
    return {'sdk/surface-java/src/main/java/org/coglet/surface/Generated.java':'\n'.join(out)+'\n', 'sdk/surface-java/src/main/resources/schema.json':json.dumps(rules,sort_keys=True,separators=(',',':'))+'\n'}
