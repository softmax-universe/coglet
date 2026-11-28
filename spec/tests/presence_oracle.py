"""Spec-only presence transition oracle, not a serving router.

Callers provide authenticated scoped authority and source binding out of band;
wire fields alone never supply these. Milliseconds belong to one durable epoch.
"""
import copy
from test_common import encode

MAX = 2**64 - 1


class Rejected(ValueError):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


class Presence:
    def __init__(self, cogent, attention, members):
        self.state = dict(cogent=cogent, attention=attention, present=[], limit=4, revision=0)
        self.members = set(members)
        self.holds = {}
        self.dropped = {}
        self.early = {}
        self.requested = {}
        self.calls = set()
        self.delivered = []
        self.clock = 0

    def now(self, ms):
        if ms < self.clock or not 0 <= ms <= MAX:
            raise Rejected('unavailable')
        self.clock = ms

    def authorize(self, params, method, grants):
        if 'reason' in params and not 1 <= len(params['reason'].encode('utf-8')) <= 1024:
            raise Rejected('invalid_argument')
        if params['cogent'] != self.state['cogent'] or method not in grants:
            raise Rejected('permission_denied')

    def set(self, params, grants):
        self.authorize(params, 'presence.set', grants)
        if any(k in params for k in ('attention', 'limit')) and 'presence.attention' not in grants:
            raise Rejected('permission_denied')
        if params['base'] != self.state['revision']:
            raise Rejected('conflict')
        add, remove = params['add'], params['remove']
        attention = params.get('attention', self.state['attention'])
        limit = params.get('limit', self.state['limit'])
        if (len(add) != len(set(add)) or len(remove) != len(set(remove)) or set(add) & set(remove)
                or not (set(add) | set(remove) | {attention}) <= self.members):
            raise Rejected('invalid_argument')
        present = (set(self.state['present']) - set(remove)) | set(add)
        if attention in present or len(present) > limit:
            raise Rejected('invalid_argument')
        next_state = dict(self.state, present=sorted(present), attention=attention, limit=limit)
        if next_state != self.state:
            if self.state['revision'] == MAX:
                raise Rejected('resource_exhausted')
            next_state['revision'] += 1
        self.state = next_state
        return copy.deepcopy(self.state)

    def request(self, params, grants, source, ms):
        self.authorize(params, 'presence.request', grants)
        if source != params['coglet'] or source not in self.members:
            raise Rejected('permission_denied')
        self.now(ms)
        if source in self.requested and ms - self.requested[source] < 60000:
            raise Rejected('resource_exhausted')
        self.requested[source] = ms
        notification = dict(target=self.state['attention'], params=copy.deepcopy(params))
        self.delivered.append(notification)
        return notification

    def end_call(self, origin_attention, sender, correlation):
        # Retirement follows the retained call identity, not today's attention.
        self.calls.discard((origin_attention, sender, correlation))

    def call(self, sender, correlation):
        # Recorded only after an authenticated attention-originated call.
        self.calls.add((self.state['attention'], sender, correlation))

    def check_in(self, source):
        result = dict(cogent=self.state['cogent'], coglet=source,
                      held=copy.deepcopy(self.holds.get(source, [])),
                      dropped=self.dropped.get(source, 0))
        self.holds[source] = []
        self.dropped[source] = 0
        return result

    def message(self, sender, target, message, ms):
        self.now(ms)
        if sender not in self.members or message['sender'] != sender or message['event']['source'] != sender:
            raise Rejected('permission_denied')
        correlation = message['event']['correlation_id']
        if (target != self.state['attention'] or sender == target or sender in self.state['present']
                or (target, sender, correlation) in self.calls):
            self.delivered.append(copy.deepcopy(message))
            return None
        if len(encode(message)) > 524288:
            raise Rejected('resource_exhausted')
        queue = self.holds.setdefault(sender, [])
        # The 64th admitted message triggers an early check-in if allowed.
        if len(queue) >= 63 and (sender not in self.early or ms - self.early[sender] >= 10000):
            self.early[sender] = ms
            if len(queue) == 64:
                result = self.check_in(sender)
                self.holds[sender].append(copy.deepcopy(message))
                return result
            queue.append(copy.deepcopy(message))
            return self.check_in(sender)
        if len(queue) == 64:
            if self.dropped.get(sender, 0) == MAX:
                raise Rejected('resource_exhausted')
            queue.pop(0)
            self.dropped[sender] = self.dropped.get(sender, 0) + 1
        queue.append(copy.deepcopy(message))
        return None
