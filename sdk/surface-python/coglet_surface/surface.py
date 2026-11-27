"""Surface-side protocol. All configuration methods are trusted supervisor APIs.

Callbacks must be bounded/nonblocking. Native stop MUST be independent of the
control queue. Serialize this object on one control thread; marshal completions
back to it. A hardware off-switch must call native stop even if that thread hangs.
"""
from collections import deque
from dataclasses import dataclass

from importlib.resources import files
import json

import time
import uuid
from typing import Callable

from .codec import decode_profile, encode_profile, validate
from .generated import SCHEMA
from .errors import ProtocolError
from .ledger import Ledger
from .catalog import TrustedExtensionCatalog, profile, name as validate_name, wire_schema

WIRE = 'surface::surface-message'
MAX_FRAME = 1_048_576
CATALOG = json.loads(files(__package__).joinpath('bindings.json').read_text())['operations']
RESOURCES = {
    'expression.set_expression': ('display.face',),
    **{name: ('audio.mic',) for name in ('speech.start', 'speech.partial', 'speech.end')},
    **{name: ('audio.speaker',) for name in ('speech.position', 'speech.speak_chunk',
                                          'speech.stop', 'speech.backchannel')},
    **{name: ('audio.mic', 'audio.speaker') for name in ('speech.barge_in', 'speech.session_open',
        'speech.session_close', 'speech.tool_call', 'speech.tool_result', 'speech.work_state',
        'speech.work_intent', 'speech.work_done', 'speech.work_result')},
}


class Rejection(Exception):
    """Side-effect-free prepare/read refusal, safe to expose as a common error."""
    def __init__(self, code, message='operation refused'):
        self.error = dict(code=code, message=message, retryable=False)
        validate('common::error', self.error)
        super().__init__(message)


def correlation_id():
    return uuid.uuid4().hex


@dataclass(frozen=True)
class RequestContext:
    coglet: str
    epoch: int
    correlation_id: str
    deadline: int
    action_id: str | None

    @classmethod
    def from_request(cls, request, epoch):
        return cls(request['coglet'], epoch, request['correlation_id'], request['deadline'], request.get('id'))


@dataclass(frozen=True)
class Operation:
    name: str
    entry: str
    direction: str
    input: str
    output: str | None
    resources: tuple[str, ...]
    prepare: Callable
    execute: Callable

    @property
    def uri(self):
        family, rule = (self.output if self.direction == 'read' else self.input).split('::')
        return f'https://coglet.dev/spec/messages/{family}.schema.json#/$defs/{rule}'


class Surface:
    def __init__(self, identity, coglets, ledger, *, create=False, clock=None, stop,
                 extensions=None, schemas=None, contract_profile='surface-0.1', trusted_catalog=None):
        stop()  # Uncertain native work must stop even if ledger/config opening fails.
        self.contract_profile = profile(contract_profile)
        if trusted_catalog is not None and (type(trusted_catalog) is not TrustedExtensionCatalog
                or trusted_catalog.contract_profile != contract_profile):
            raise ValueError('trusted catalog/profile mismatch')
        self.trusted_catalog = trusted_catalog
        self._trusted = trusted_catalog.artifact if trusted_catalog else None
        self.version = '0.1'
        self.catalog = dict(CATALOG)
        for name, binding in (extensions or {}).items():
            validate_name(name, 'surface-0.1')
            if name in CATALOG or name.startswith(('runtime.', 'llm.', 'action.')):
                raise ValueError('reserved or built-in binding override')
            self.catalog[name] = dict(binding)
        if set(schemas or {}).intersection(SCHEMA):
            raise ValueError('built-in schema override')
        self.schemas = {**SCHEMA, **(schemas or {})}
        if self._trusted:
            if set(extensions or {}).intersection(self._trusted['operations']):
                raise ValueError('pinned binding override')
            for name, binding in self._trusted['operations'].items():
                self.catalog[name] = {**binding, 'output': binding.get('output')}
        validate('common::surface-id', identity)
        for coglet in coglets:
            validate('common::coglet-id', coglet)
        self.identity = identity
        self.coglets = frozenset(coglets)
        self.clock = clock or (lambda: time.monotonic_ns() // 1000)
        self.stop = stop
        self.state = 'closed'
        self.operations = {}
        self.grants = []
        self.queue = deque()
        self.last_received = self.last_health = 0
        self.read_ids = set()
        self.revocation = 0
        self.revision = 1
        self.ledger = Ledger(ledger, identity, create, contract_profile=contract_profile, catalog=trusted_catalog)
        assert self.ledger.db is not None
        self.db = self.ledger.db
        self.last_clock = self._get('clock')
        self.closed = False
        try:
            self._recover()
        except BaseException:
            self.ledger.close()
            raise

    def _recover(self):
        # Uncertain work must be physically stopped before recovery receipts exist.
        for coglet, aid, envelope in self.db.execute(
                'SELECT coglet,id,envelope FROM actions WHERE outcome IS NULL').fetchall():
            receipt = self.db.execute('SELECT receipt FROM actions WHERE coglet=? AND id=?',
                                      (coglet, aid)).fetchone()[0]
            if decode_profile(receipt)['accepted']:
                action = self._decode_saved(envelope)['body']['action']
                outcome = self._outcome(action, 'failed', error=Rejection('unavailable').error)
                self.db.execute('UPDATE actions SET outcome=? WHERE coglet=? AND id=?',
                                (encode_profile(outcome), coglet, aid))
        self.db.commit()

    def _get(self, key, default=0):
        row = self.db.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()
        return int(row[0]) if row else default

    def _set(self, key, value):
        row = self.db.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()
        if row and row[0] == str(value):
            return
        with self.db:
            self.db.execute('INSERT OR REPLACE INTO meta VALUES (?,?)', (key, str(value)))

    def now(self):
        now = self._sample()
        self._set('clock', now)
        return now

    def _sample(self):
        """Fresh monotonic sample; absolutely no blocking storage on this path."""
        now = self.clock()
        validate('common::uint64', now)
        if now < self.last_clock:
            self._abort()
            raise ProtocolError('clock rollback')
        self.last_clock = now
        return now

    def _message(self, kind, body):
        message = {'type': kind, 'body': body}
        validate(WIRE, message, wire_schema(self.version))
        raw = encode_profile(message)
        if len(raw) > MAX_FRAME:
            raise ProtocolError('frame too large')
        return raw

    def _emit(self, kind, body):
        try:
            raw = self._message(kind, body)
        except Exception:
            self._abort()
            raise
        if len(self.queue) >= 128:
            self._abort()
            raise ProtocolError('resource_exhausted: control queue overflow')
        self.queue.append(raw)

    def drain(self):
        result = list(self.queue)
        self.queue.clear()
        return result

    def register(self, operation):
        if self.state != 'closed' or operation.name in self.operations:
            raise ValueError('register unique operations before open')
        validate_name(operation.name, self.contract_profile)
        validate_name(operation.entry, 'surface-0.1')
        if operation.name.startswith(('runtime.', 'test.')):
            raise ValueError('reserved operation')
        binding = self.catalog.get(operation.name)
        pinned = self._trusted and self._trusted['operations'].get(operation.name)
        if operation.name.count('.') == 2 and not pinned:
            raise ValueError('app operations require pinned catalog')
        expected = None if binding is None else (
            binding['entry'], binding['direction'],
            binding['input'] if pinned else binding['input'].replace(':', '::'),
            binding.get('output') if pinned else (binding['output'].replace(':', '::') if binding['output'] else None))
        if (operation.entry, operation.direction, operation.input, operation.output) != expected:
            raise ValueError('operation binding mismatch or unsupported extension')
        if operation.direction not in ('action', 'read', 'event'):
            raise ValueError('invalid direction')
        required = pinned['resources'] if pinned else RESOURCES.get(operation.name, ())
        for resource in operation.resources:
            validate_name(resource, 'surface-0.1')
        # Speech I/O is deliberately not implemented by the core. Adapters must
        # declare their resources and enforce stream/session ownership in prepare.
        if not set(required) <= set(operation.resources) or (pinned and set(required) != set(operation.resources)):
            raise ValueError('missing binding resource')
        self.operations[operation.name] = operation

    def open(self, cid=None):
        if self.closed or self.state != 'closed':
            raise ProtocolError('already open')
        self.revision = 1
        self.read_ids.clear()
        self._set('peer_clock', 0)
        self.cid = cid or correlation_id()
        raw = self._message('hello', {'surface': self.identity,
                            'offer': {'versions': ['0.2', '0.1'] if self.contract_profile == 'surface-0.2' else ['0.1']}, 'clock': self.now(),
                            'correlation_id': self.cid})
        self.state = 'negotiating'
        self.last_received = self.last_health = self.now()
        return raw

    def _capabilities(self):
        entries = {}
        for op in self.operations.values():
            if not self._available(op.name):
                continue
            entry = entries.setdefault(op.entry, dict(name=op.entry, events=[], reads=[],
                                      actions=[], exclusive=[], schemas={}))
            entry[{'event': 'events', 'read': 'reads', 'action': 'actions'}[op.direction]].append(op.name)
            entry['exclusive'] = sorted(set(entry['exclusive']) | set(op.resources))
            entry['schemas'][op.name] = ((op.output if op.direction == 'read' else op.input)
                if self._trusted and op.name in self._trusted['operations'] else op.uri)
        self._emit('capabilities', dict(revision=self.revision, clock=self.now(),
                    entries=list(entries.values()), correlation_id=correlation_id()))

    def grant(self, entries):
        if self.state != 'ready':
            raise ProtocolError('grants require negotiation')
        body = dict(epoch=self._get('epoch') + 1, entries=entries, correlation_id=correlation_id())
        validate('surface::grants', body, wire_schema(self.version))
        owners, seen = set(), set()
        now = self.now()
        for grant in entries:
            for resource in grant['resources']:
                validate_name(resource, 'surface-0.1')
            if (grant['coglet'] not in self.coglets or grant['coglet'] in seen
                    or grant['expires'] <= now
                    or not set(grant['operations']) <= self.operations.keys()
                    or any(not self._available(n) for n in grant['operations'])
                    or len(grant['resources']) != len(set(grant['resources']))
                    or owners.intersection(grant['resources'])):
                raise ValueError('invalid grant snapshot')
            owners.update(grant['resources'])
            seen.add(grant['coglet'])
        if self._get('killed') and entries:
            # Keep the latch through reconnect. Never reinstall authority.
            entries = []
            body['entries'] = []
        if self.grants:
            self.stop()
            self._cancel('cancelled', 'cancelled')
            self.revocation += 1
            self.queue = deque(raw for raw in self.queue if not private_frame(raw))
        self._set('epoch', body['epoch'])
        self.grants = decode_profile(encode_profile(entries))
        self._emit('grants', body)

    def receive(self, raw):
        try:
            self._receive(raw)
        except Exception:
            self._abort()
            raise

    def _receive(self, raw):
        try:
            if not 0 < len(raw) <= MAX_FRAME:
                raise ValueError('frame size')
            message = decode_profile(raw)
            validate(WIRE, message, wire_schema(self.version))
        except ValueError as exc:
            raise ProtocolError('invalid surface frame') from exc
        body, kind = message['body'], message['type']
        self.last_received = self.now()
        if self.state == 'negotiating':
            if (kind != 'hello_reply' or body['correlation_id'] != self.cid
                    or body['selection']['version'] not in (
                        ('0.1', '0.2') if self.contract_profile == 'surface-0.2' else ('0.1',))):
                raise ProtocolError('invalid negotiation')
            self.version = body['selection']['version']
            self.state = 'ready'
            self._capabilities()
        elif self.state == 'ready' and kind == 'act':
            self._act(raw, body)
        elif self.state == 'ready' and kind == 'read':
            self._read(body)
        elif self.state == 'ready' and kind == 'health':
            if body['clock'] < self._get('peer_clock'):
                raise ProtocolError('peer clock rollback')
            self._set('peer_clock', body['clock'])
        else:
            raise ProtocolError('invalid direction or lifecycle')

    def _admit(self, request, epoch, direction, *, prepare=True):
        if request['coglet'] not in self.coglets:
            raise Rejection('permission_denied')
        if self._get('killed'):
            raise Rejection('permission_denied')
        now = self.now()
        op = self.operations.get(request['capability'])
        if epoch != self._get('epoch'):
            raise Rejection('permission_denied')
        grant = next((g for g in self.grants if g['coglet'] == request['coglet']), None)
        if not grant or grant['expires'] <= now or request['capability'] not in grant['operations']:
            raise Rejection('permission_denied')
        if op is None or op.direction != direction or not self._available(op.name):
            raise Rejection('invalid_argument')
        if not set(op.resources) <= set(grant['resources']):
            raise Rejection('permission_denied')
        if request['deadline'] <= now:
            raise Rejection('deadline_exceeded')
        if request['deadline'] > now + 60_000_000:
            raise Rejection('invalid_argument')
        try:
            self._validate_operation(op, False, request['args'])
        except ValueError as exc:
            raise Rejection('invalid_argument') from exc
        if prepare:
            op.prepare(request['args'], RequestContext.from_request(request, epoch))
        self._execution_window(request, epoch)
        return op

    def _execution_window(self, request, epoch):
        # Fetch all storage/state BEFORE sampling, never checkpoint a clock
        # between this final sample and the adapter's native handoff.
        current_epoch, killed = self._get('epoch'), self._get('killed')
        grant = next((g for g in self.grants if g['coglet'] == request['coglet']), None)
        now = self._sample()
        if (self.state != 'ready' or killed or epoch != current_epoch or not grant
                or grant['expires'] <= now or request['capability'] not in grant['operations']):
            raise Rejection('permission_denied')
        if request['deadline'] <= now:
            raise Rejection('deadline_exceeded')

    def _act(self, raw, body):
        action = body['action']
        if not self._available(action['capability']):
            self._emit('accept', self._receipt(action, Rejection('permission_denied').error))
            return
        key = (action['coglet'], action['id'])
        if action['coglet'] not in self.coglets:
            self._emit('accept', self._receipt(action, Rejection('permission_denied').error))
            return
        previous = self.db.execute('SELECT envelope,receipt,outcome FROM actions '
                                   'WHERE coglet=? AND id=?', key).fetchone()
        if previous:
            if previous[0] != raw:
                self._emit('accept', self._receipt(action, Rejection('conflict').error))
            else:
                self._emit('accept', decode_profile(previous[1]))
                if previous[2] is not None:
                    self._event(decode_profile(previous[2]))
            return
        try:
            op = self._admit(action, body['grant_epoch'], 'action')
            if len(self.queue) > 124 or len(self._pending()) >= 64:
                raise Rejection('resource_exhausted')
            receipt = self._receipt(action)
        except Rejection as exc:
            receipt = self._receipt(action, exc.error)
        with self.db:
            self.db.execute('INSERT INTO actions VALUES (?,?,?,?,NULL)',
                            (*key, raw, encode_profile(receipt)))
        self._emit('accept', receipt)
        if receipt['accepted']:
            try:
                # SQLite durability and output serialization also consume time.
                # A committed admission may expire, but must never start an effect.
                self._execution_window(action, body['grant_epoch'])
            except Rejection as exc:
                self.stop()
                status = 'expired' if exc.error['code'] == 'deadline_exceeded' else 'cancelled'
                self.complete(*key, status=status, error=exc.error)
                return
            try:
                result = op.execute(action['args'], RequestContext.from_request(action, body['grant_epoch']))
                if result is not None:
                    self.complete(*key, result=result)
            except ProtocolError:
                raise
            except Exception:
                self.stop()
                self.complete(*key, status='failed', error=Rejection('internal').error)

    @staticmethod
    def _receipt(action, error=None):
        result = dict(id=action['id'], correlation_id=action['correlation_id'], accepted=error is None)
        if error:
            result['error'] = error
        return result

    @staticmethod
    def _outcome(action, status, **fields):
        return dict(id=action['id'], correlation_id=action['correlation_id'], status=status, **fields)

    def complete(self, coglet, aid, *, status='succeeded', result=None, error=None):
        row = self.db.execute('SELECT envelope,receipt,outcome FROM actions WHERE coglet=? AND id=?',
                              (coglet, aid)).fetchone()
        if not row or not decode_profile(row[1])['accepted'] or row[2] is not None:
            return False
        action = self._decode_saved(row[0])['body']['action']
        grant = next((g for g in self.grants if g['coglet'] == coglet), None)
        if status == 'succeeded' and (self._get('killed') or not grant
                or grant['expires'] <= self.now()
                or not self._available(action['capability'])
                or self._decode_saved(row[0])['body']['grant_epoch'] != self._get('epoch')):
            self.stop()
            status, error = 'cancelled', Rejection('permission_denied').error
        if status == 'succeeded' and self.now() >= action['deadline']:
            self.stop()
            status, error = 'expired', Rejection('deadline_exceeded').error
        if status == 'succeeded':
            self._validate_operation(self.operations[action['capability']], True, result)
            outcome = self._outcome(action, status, result=result)
        else:
            outcome = self._outcome(action, status, error=error)
        validate('common::action-outcome', outcome)
        with self.db:
            self.db.execute('UPDATE actions SET outcome=? WHERE coglet=? AND id=?',
                            (encode_profile(outcome), coglet, aid))
        self._event(outcome)
        return True

    def _event(self, outcome):
        seq = self._get('seq', -1) + 1
        self._set('seq', seq)
        self._emit('event', dict(source=self.identity, kind='action.outcome', seq=seq,
                   logical_time=self.now(), payload=outcome, correlation_id=outcome['correlation_id']))

    def _read(self, request):
        cid = request['correlation_id']
        if cid in self.read_ids:
            raise ProtocolError('reused read correlation')
        if len(self.read_ids) >= 65536:
            raise ProtocolError('resource_exhausted: read correlation retention')
        self.read_ids.add(cid)
        try:
            op = self._admit(request, request['grant_epoch'], 'read')
            value = op.execute(request['args'], RequestContext.from_request(request, request['grant_epoch']))
            self._validate_operation(op, True, value)
            self._execution_window(request, request['grant_epoch'])
            if op.name == 'speech.heard' and value['item'] != request['args']['item']:
                raise ValueError('heard item mismatch')
            revision = self._get('snapshot') + 1
            self._set('snapshot', revision)
            reply = dict(correlation_id=cid, ok=True, value=value, snapshot=revision)
        except Rejection as exc:
            reply = dict(correlation_id=cid, ok=False, error=exc.error)
        except Exception:
            reply = dict(correlation_id=cid, ok=False, error=Rejection('internal').error)
        self._emit('read_reply', reply)

    def event(self, coglet, name, payload, *, epoch):
        """Emit a bound observation. Runtime still intersects subscriptions/grants.

        For private streams the adapter supplies the retained session owner, never
        a caller-chosen owner, and fences late callbacks by session ID and epoch.
        """
        if self.state != 'ready' or self._get('killed') or epoch != self._get('epoch'):
            raise Rejection('permission_denied')
        op = self.operations.get(name)
        grant = next((g for g in self.grants if g['coglet'] == coglet), None)
        if (not op or op.direction != 'event' or not grant or grant['expires'] <= self.now()
                or not self._available(name)
                or name not in grant['operations'] or not set(op.resources) <= set(grant['resources'])):
            raise Rejection('permission_denied')
        self._validate_operation(op, False, payload)
        selected = self._trusted and self._trusted['operations'].get(name, {}).get('selection')
        if selected:
            read = payload.get(selected['operation_field'])
            if read not in selected['reads'] or read not in grant['operations'] or not self._available(read):
                raise Rejection('permission_denied')
            read_op = self.operations.get(read)
            if not read_op or not set(read_op.resources) <= set(grant['resources']):
                raise Rejection('permission_denied')
            self._validate_operation(read_op, False, payload.get(selected['args_field']))
            self._validate_operation(read_op, True, payload.get(selected['value_field']))
        seq = self._get('seq', -1) + 1
        self._set('seq', seq)
        self._emit('event', dict(source=self.identity, kind=name, seq=seq,
            logical_time=self.now(), payload=payload, correlation_id=correlation_id()))

    def remove(self, name):
        if self.state != 'ready' or name not in self.operations:
            raise ValueError('unknown operation or lifecycle')
        entries = []
        for grant in self.grants:
            remaining = [op for op in grant['operations'] if op != name]
            if remaining:
                entries.append({**grant, 'operations': remaining})
        self.grant(entries)
        del self.operations[name]
        self.revision += 1
        self._capabilities()

    def _cancel(self, status, code):
        for coglet, aid, _ in self._pending():
            self.complete(coglet, aid, status=status, error=Rejection(code).error)

    def _pending(self):
        return [(coglet, aid, raw) for coglet, aid, raw, receipt in self.db.execute(
            'SELECT coglet,id,envelope,receipt FROM actions WHERE outcome IS NULL').fetchall()
            if decode_profile(receipt)['accepted']]

    def health(self):
        pending = self.db.execute('SELECT receipt FROM actions WHERE outcome IS NULL').fetchall()
        self._emit('health', dict(state='stopped' if self._get('killed') else 'ready',
            clock=self.now(), pending=sum(decode_profile(r[0])['accepted'] for r in pending),
            correlation_id=correlation_id()))

    def tick(self):
        now = self.now()
        if now - self.last_received >= 3_000_000:
            self.disconnect()
            raise ProtocolError('peer heartbeat timeout')
        if any(g['expires'] <= now for g in self.grants):
            self.grant([g for g in self.grants if g['expires'] > now])
        for coglet, aid, raw in self._pending():
            if self._decode_saved(raw)['body']['action']['deadline'] <= now:
                self.stop()
                self.complete(coglet, aid, status='expired', error=Rejection('deadline_exceeded').error)
        if self.state == 'ready' and now - self.last_health >= 1_000_000:
            self.health()
            self.last_health = now

    def kill(self, reason):
        self.stop()  # Reflex first, never wait on a queue or peer.
        self._set('killed', 1)
        self.grants.clear()
        self._set('epoch', self._get('epoch') + 1)
        self.revocation += 1
        self.queue = deque(raw for raw in self.queue if not private_frame(raw) and not authority_frame(raw))
        self._cancel('cancelled', 'cancelled')
        terminal = self.drain()
        self._emit('kill', dict(reason=reason, correlation_id=correlation_id()))
        self.health()
        self.queue.extend(terminal)

    def reset(self):
        if self.state != 'closed':
            raise ProtocolError('local reset requires closed connection')
        self._set('killed', 0)

    def _abort(self):
        self.stop()
        self.state = 'closed'
        self.grants.clear()
        self.revocation += 1
        self.queue.clear()
        # No clock or output queue is usable on the fatal path. Commit terminal
        # cancellation before reconciliation, without manufacturing a delivery.
        with self.db:
            for coglet, aid, raw in self._pending():
                action = self._decode_saved(raw)['body']['action']
                outcome = self._outcome(action, 'cancelled', error=Rejection('unavailable').error)
                self.db.execute('UPDATE actions SET outcome=? WHERE coglet=? AND id=?',
                                (encode_profile(outcome), coglet, aid))

    def disconnect(self):
        self.stop()
        self.revocation += 1
        self.queue.clear()
        self._cancel('cancelled', 'cancelled')
        self.state = 'closed'
        self.grants.clear()

    def _available(self, name):
        return not (self.contract_profile == 'surface-0.2' and self.version == '0.1' and name not in CATALOG)

    def _decode_saved(self, raw):
        message = decode_profile(raw)
        validate(WIRE, message, wire_schema('0.2' if self.contract_profile == 'surface-0.2' else '0.1'))
        return message

    def _validate_operation(self, op, output, value):
        if self._trusted and op.name in self._trusted['operations']:
            validate(self._trusted['schemas'][op.output if output else op.input], value, self._trusted['rules'])
        else:
            validate(op.output if output else op.input, value, self.schemas)

    def close(self):
        if not self.closed:
            try:
                self.disconnect()
            finally:
                self.ledger.close()
                self.closed = True


def private_frame(raw):
    message = decode_profile(raw)
    return message['type'] == 'read_reply' or (message['type'] == 'event'
        and message['body']['kind'] != 'action.outcome')


def authority_frame(raw):
    return decode_profile(raw)['type'] in ('grants', 'capabilities')
