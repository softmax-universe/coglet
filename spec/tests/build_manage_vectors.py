"""Generate filesystem patch, signing and restart-tail vectors; spec-only."""
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from test_common import encode
from manage_oracle import tree, signing, log_bytes

OUT = Path(__file__).resolve().parents[1] / 'fixtures/hub'


def write(name, value):
    (OUT / name).write_text(json.dumps(value, indent=2) + '\n')


with tempfile.TemporaryDirectory() as td:
    p = Path(td)
    def key(name, seed):
        (p / name).write_bytes(bytes.fromhex('302e020100300506032b657004220420' + seed * 32))
        return subprocess.check_output(['openssl', 'pkey', '-inform', 'DER', '-in', str(p / name), '-pubout', '-outform', 'DER'])[-32:].hex()
    cog, hub, device = [key(n, seed) for n, seed in [('cog', '01'), ('hub', '02'), ('device', '03')]]
    cid = 'coglet:' + cog
    def sign(raw, name):
        (p / 'message').write_bytes(raw)
        return subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-keyform', 'DER', '-inkey', str(p / name), '-rawin', '-in', str(p / 'message')]).hex()
    module = bytes.fromhex('0061736d01000000')
    source = {'bin/step.wasm': module.hex(), 'manifest': '7b7d0a'}
    inputs = {'recipe': b'fixture recipe v1', 'policy': b'test-only no-op policy', 'transform': b'test-only identity transform'}
    record = dict(version='2.0', source_tree=tree(source),
        **{n + '_sha256': hashlib.sha256(v).hexdigest() for n, v in inputs.items()},
        original_sha256=hashlib.sha256(module).hexdigest(), rewritten_sha256=hashlib.sha256(module).hexdigest())
    files = dict(source, **{'build/provenance.cbor': encode(record).hex()})
    patch = {'revision': tree(files), 'generation': 1,
             'files': [{'path': path, 'bytes': files[path]} for path in sorted(files)]}
    patch['signature'] = sign(signing(patch, hub, cid), 'hub')
    statement = dict(purpose='delegation', hub=hub, coglet=cid, generation=1,
        device=device, expires=4102444800,
        methods=['hub.install', 'manage.patch', 'hub.status', 'hub.events', 'hub.actions'])
    raw = b'coglet/hub/0:delegation\n' + encode(statement)
    write('signatures.json', [{'statement': statement, 'signer': cog, 'signature': sign(raw, 'cog'), 'preimage_hex': raw.hex()}])
    write('patch.json', {'hub': hub, 'coglet': cid, 'patch': patch,
        'preimage_hex': signing(patch, hub, cid).hex(), 'files': files,
        'source_files': source, 'build_inputs': {k: v.hex() for k, v in inputs.items()}, 'original_hex': module.hex()})
    valid = dict(patch, base=tree(files), generation=2)
    cases = []
    for name, code, reason in [('stale-base', 'conflict', 'stale_base'),
        ('equal-generation', 'conflict', 'generation'), ('lower-generation', 'conflict', 'generation'),
        ('bad-path', 'invalid_argument', 'path'), ('missing-executable', 'invalid_argument', 'executable')]:
        value = copy.deepcopy(valid)
        if name == 'stale-base': value['base'] = '0' * 64
        if name == 'equal-generation': value['generation'] = 1
        if name == 'lower-generation': value['generation'] = 0
        if name == 'bad-path': value['files'][0]['path'] = '../escape'
        if name == 'missing-executable':
            value['files'] = [{'path': 'bin/step.wasm'}]
            value['revision'] = tree({k: v for k, v in files.items() if k != 'bin/step.wasm'})
        value['signature'] = sign(signing(value, hub, cid), 'hub')
        cases.append(dict(name=name, patch=value, code=code, reason=reason))
    write('patch-rejections.json', cases)
    first = log_bytes('/logs/stdout', 0, b'first\n')
    second = log_bytes('/logs/stderr', 0, b'next\n')
    write('tail-restart.json', {'before_hex': first.hex(), 'after_hex': second.hex(), 'restart_offset': len(first)})
for name in ['patch.json', 'patch-rejections.json', 'tail-restart.json']:
    (OUT / (name + '.format')).write_text('semantic-vectors-v1\n')
print('Generated filesystem patch, rejections, signing and restart-tail vectors')
