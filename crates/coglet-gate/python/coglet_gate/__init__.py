"""Trusted-local native sandbox evidence, not production build qualification.

Keys and tool identities are operator-pinned, never accepted from a factory.
HMAC keys imply symmetric trust: verifiers must be as trusted as issuers.
The SQLite file contains ONLY gate challenges, never router promotion state.
"""
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from hashlib import sha256
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import sqlite3
import sys
import tempfile


class Refused(ValueError):
    """Untrusted, mismatched, unavailable or already-consumed evidence."""


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'),
                      ensure_ascii=True, allow_nan=False).encode('ascii')


def digest(value):
    return sha256(value).hexdigest()


def hex_value(value, lengths):
    return isinstance(value, str) and len(value) in lengths and re.fullmatch('[0-9a-f]+', value)


@dataclass(frozen=True)
class TrustKey:
    identity: str
    secret: bytes

    def __post_init__(self):
        if not isinstance(self.identity, str) or not re.fullmatch('[a-zA-Z0-9._-]{1,128}', self.identity):
            raise Refused('invalid key identity')
        if not isinstance(self.secret, bytes) or len(self.secret) < 32:
            raise Refused('at least 256 bits of operator key material required')

    def mac(self, kind, payload):
        return hmac.new(self.secret, b'coglet-gate-v1\0' + kind.encode('ascii') + b'\0'
                        + canonical(payload), sha256).hexdigest()


@dataclass(frozen=True)
class Binding:
    coglet: str
    repo: str
    candidate: str
    base: str | None
    revision: str
    rewritten_sha256: str

    def __post_init__(self):
        if (not isinstance(self.coglet, str) or not re.fullmatch('coglet:[0-9a-f]{64}', self.coglet)
                or not isinstance(self.repo, str) or not re.fullmatch('[a-zA-Z0-9._:/-]{1,4096}', self.repo)
                or '://' in self.repo or not hex_value(self.candidate, (40, 64))
                or (self.base is not None and not hex_value(self.base, (len(self.candidate),)))
                or not hex_value(self.revision, (32,))
                or not hex_value(self.rewritten_sha256, (64,))):
            raise Refused('invalid exact build binding')

    def document(self):
        return asdict(self)


@dataclass(frozen=True)
class Suite:
    suite: str
    world: str
    seed: int
    max_steps: int
    minimum_actions: int

    def __post_init__(self):
        if (not isinstance(self.suite, str) or not re.fullmatch('[a-zA-Z0-9._-]{1,128}', self.suite)
                or self.world not in ('gridbot', 'cartpole')
                or type(self.seed) is not int or not 0 <= self.seed < 2**64
                or type(self.max_steps) is not int or not 1 <= self.max_steps <= 1024
                or type(self.minimum_actions) is not int or not 1 <= self.minimum_actions <= 1000000):
            raise Refused('invalid bounded suite configuration')

    def document(self):
        return asdict(self)


class ArtifactStore:
    """Immutable-by-API, content-addressed authenticated JSON; mutations fail verification.

    This directory is authority-owned. Filesystem administrators are inside the
    trust boundary, not protected by chmod; authentication still detects edits.
    """
    def __init__(self, root):
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)

    def put(self, payload, key, kind):
        raw = canonical(dict(version=1, issuer=key.identity, kind=kind,
                             payload=payload, mac=key.mac(kind, payload)))
        if len(raw) > 8 * 1024 * 1024:
            raise Refused('artifact too large')
        ref = digest(raw)
        # Publish complete bytes atomically, without replacement of an existing ref.
        fd, name = tempfile.mkstemp(prefix='.artifact-', dir=self.root)
        try:
            with os.fdopen(fd, 'wb') as stream:
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
                os.fchmod(stream.fileno(), 0o400)
            try:
                os.link(name, self.root / ref)
            except FileExistsError:
                pass
            if (self.root / ref).read_bytes() != raw:
                raise Refused('immutable artifact collision or mutation')
            directory = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        finally:
            os.unlink(name)
        return ref

    def read(self, ref, key, kind):
        if not hex_value(ref, (64,)):
            raise Refused('invalid artifact reference')
        try:
            with (self.root / ref).open('rb') as stream:
                raw = stream.read(8 * 1024 * 1024 + 1)
            value = json.loads(raw)
            if (len(raw) > 8 * 1024 * 1024 or digest(raw) != ref or canonical(value) != raw
                    or set(value) != {'version', 'issuer', 'kind', 'payload', 'mac'}
                    or type(value['version']) is not int or value['version'] != 1
                    or value['issuer'] != key.identity or value['kind'] != kind
                    or not isinstance(value['mac'], str)
                    or not hmac.compare_digest(value['mac'], key.mac(kind, value['payload']))):
                raise Refused('artifact authentication failed')
            return value['payload']
        except (OSError, ValueError, TypeError, KeyError) as error:
            raise Refused('artifact authentication failed') from error


class BuildIssuer:
    """Authenticate M2 testbed bytes WITHOUT asserting production build authority.

    No production issuer exists here. M2 provenance is not a build attestation.
    """
    def __init__(self, store, key):
        self.store, self.key = store, key

    def fixture(self, binding, rewritten, provenance):
        if not isinstance(rewritten, bytes) or not rewritten or digest(rewritten) != binding.rewritten_sha256:
            raise Refused('rewritten bytes mismatch')
        if not isinstance(provenance, bytes) or not provenance:
            raise Refused('explicit fixture provenance required')
        return self.store.put(dict(binding=binding.document(), qualification='fixture-only',
                                   provenance_sha256=digest(provenance)), self.key, 'build')


def read_build(store, key, ref, binding):
    value = store.read(ref, key, 'build')
    if (not isinstance(value, dict) or set(value) != {'binding', 'qualification', 'provenance_sha256'}
            or value['binding'] != binding.document() or value['qualification'] != 'fixture-only'
            or not hex_value(value['provenance_sha256'], (64,))):
        raise Refused('build binding or qualification mismatch')
    return value


def validate_request(request, key):
    try:
        if set(request) != {'nonce', 'binding', 'suite', 'build', 'tools', 'mac'}:
            raise Refused('invalid request fields')
        body = {k: v for k, v in request.items() if k != 'mac'}
        if not hex_value(body['nonce'], (64,)) or not hex_value(body['build'], (64,)):
            raise Refused('invalid request identity')
        if not hmac.compare_digest(request['mac'], key.mac('challenge', body)):
            raise Refused('request authentication failed')
        return Binding(**body['binding']), Suite(**body['suite'])
    except (TypeError, KeyError) as error:
        raise Refused('invalid request') from error


class GateVerifier:
    """Authority-owned one-use evaluation challenge admission, restart-safe.

    accept_fixture() consumes a challenge even for a behavioral rejection. Revalidation
    of a retained artifact is separate from acceptance; never call accept_fixture() as
    the router's repeatable GateAuthority.verify hook. The router owns its own
    receipt/evidence association. A caller cannot reset consumed nonces.
    Production is deliberately unavailable until a real build producer qualifies.
    """
    def __init__(self, database, store, build_key, gate_key, *, tools=None):
        if build_key.identity == gate_key.identity or build_key.secret == gate_key.secret:
            raise Refused('separate build and gate identities required')
        self.database = Path(database)
        self.store, self.build_key, self.gate_key = store, build_key, gate_key
        # None means no native evidence can be accepted, not wildcard tool trust.
        self.tools = None if tools is None else dict(tools)
        with self._connect() as connection:
            connection.execute('CREATE TABLE IF NOT EXISTS challenges '
                               '(nonce TEXT PRIMARY KEY, request BLOB NOT NULL, artifact TEXT)')

    @contextmanager
    def _connect(self):
        connection = sqlite3.connect(self.database, timeout=5)
        connection.execute('PRAGMA journal_mode=WAL')
        connection.execute('PRAGMA synchronous=FULL')
        try:
            with connection:
                yield connection
        finally:
            connection.close()

    def verify_production(self, *args, **kwargs):
        raise Refused('production build authority unavailable; fixture-only evidence')

    def challenge_fixture(self, binding, suite, build):
        read_build(self.store, self.build_key, build, binding)
        if self.tools is None:
            raise Refused('operator-pinned tool identities required')
        body = dict(nonce=secrets.token_hex(32), binding=binding.document(), suite=suite.document(),
                    build=build, tools=self.tools)
        request = {**body, 'mac': self.gate_key.mac('challenge', body)}
        with self._connect() as connection:
            connection.execute('INSERT INTO challenges VALUES (?, ?, NULL)',
                               (body['nonce'], canonical(request)))
        return json.loads(canonical(request))

    def accept_fixture(self, request, artifact):
        binding, suite = validate_request(request, self.gate_key)
        read_build(self.store, self.build_key, request['build'], binding)
        value = self.store.read(artifact, self.gate_key, 'evaluation')
        if (not isinstance(value, dict) or set(value) != {'request', 'tools', 'report', 'qualification'}
                or value['request'] != request or value['qualification'] != 'fixture-only'
                or self.tools is None or value['tools'] != self.tools or request['tools'] != self.tools
                or not isinstance(value['report'], dict) or type(value['report'].get('passed')) is not bool):
            raise Refused('evaluation binding, tool identity or qualification mismatch')
        report = value['report']
        if (report.get('rewritten_sha256') != binding.rewritten_sha256
                or report.get('independent_guest_reexecution') is not True
                or any(report.get(k) != suite.document()[k]
                       for k in ('world', 'seed', 'max_steps', 'minimum_actions'))):
            raise Refused('report configuration or replay mismatch')
        # Exact stored request plus an atomic compare-and-set prevents concurrent
        # consumers and survives a verifier restart. No promotion state is stored.
        with self._connect() as connection:
            changed = connection.execute('UPDATE challenges SET artifact=? WHERE nonce=? '
                                         'AND request=? AND artifact IS NULL',
                                         (artifact, request['nonce'], canonical(request))).rowcount
            if changed != 1:
                raise Refused('unknown challenge or replay refused')
        return value


class GateService:
    """Only actual daemon execution/reexecution can produce service evaluations.

    Construct in the operator-owned sandbox process, never grant these keys or
    executable selection to a factory. Native cooperative tests are not OS
    isolation: deploy this process inside a separately restricted sandbox.
    """
    def __init__(self, store, build_key, gate_key, tools):
        self.store, self.build_key, self.gate_key, self.tools = store, build_key, gate_key, tools
        self.tool_identity = {'daemon': digest(tools.daemon.read_bytes()),
                              'proof': digest(tools.proof.read_bytes())}
        # Pin adapter/evaluator/SDK Python source as well as native tools. A suite
        # label alone cannot authorize a changed world or assertion implementation.
        import coglet_sim
        import coglet_surface
        source = []
        for package in (coglet_sim, coglet_surface):
            root = Path(package.__file__).parent
            source.extend((package.__name__ + '/' + str(path.relative_to(root)), digest(path.read_bytes()))
                          for path in sorted(root.rglob('*.py')))
        source.append(('coglet_gate', digest(Path(__file__).read_bytes())))
        from coglet_sim.evaluation import ROOT
        source.append(('abi-fixture-setup', digest((ROOT / 'conformance/abi/cases.json').read_bytes())))
        source.append(('python', sys.version))
        self.tool_identity['adapter'] = digest(canonical(source))

    def evaluate(self, work, request, rewritten):
        # Snapshot mutable caller input before any external execution.
        request = json.loads(canonical(request))
        binding, suite = validate_request(request, self.gate_key)
        read_build(self.store, self.build_key, request['build'], binding)
        if not isinstance(rewritten, bytes) or digest(rewritten) != binding.rewritten_sha256:
            raise Refused('rewritten bytes mismatch')
        if request['tools'] != self.tool_identity:
            raise Refused('operator-pinned tool identities mismatch')
        from coglet_sim.evaluation import NativeTools, evaluate
        # Execute private copies of the pinned native binaries, closing the path
        # replacement race between hashing and Popen. No live world binding.
        with tempfile.TemporaryDirectory(prefix='gate-tools-', dir=work) as private:
            paths = []
            for name in ('daemon', 'proof'):
                target = Path(private) / name
                shutil.copyfile(getattr(self.tools, name), target)
                if digest(target.read_bytes()) != self.tool_identity[name]:
                    raise Refused('native tool changed')
                target.chmod(0o500)
                paths.append(target.resolve())
            report = evaluate(Path(work), NativeTools(*paths), rewritten, world=suite.world,
                              seed=suite.seed, max_steps=suite.max_steps,
                              minimum_actions=suite.minimum_actions)
        if (report['rewritten_sha256'] != binding.rewritten_sha256
                or any(report[k] != suite.document()[k] for k in ('world', 'seed', 'max_steps', 'minimum_actions'))):
            raise Refused('native evaluator returned mismatched configuration')
        return self.store.put(dict(request=request, tools=self.tool_identity, report=report,
                                   qualification='fixture-only'), self.gate_key, 'evaluation')
