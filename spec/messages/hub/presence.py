"""Additive router 1.1 shapes implementing ADR-0036.

Called by generate.py; common definitions are imported, never copied.
"""


def shapes(add, obj, enum, array, text, bounded, union):
    text('presence-cogent-id', '^[c]ogent:[0-9a-f]{64}$', 'cogent:' + 'd' * 64)
    bounded('presence-limit', 0, 255)
    bounded('presence-cadence-ms', 10000, 86400000)
    # Admission enforces uniqueness and total encoded-byte limits separately.
    array('presence-coglets', 'coglet-id', 255)
    add('presence-reason', 'tstr .size (1..1024)',
        {'type': 'string', 'minLength': 1, 'maxLength': 1024}, 'attention requested')
    obj('presence', {'cogent': 'presence-cogent-id', 'attention': 'coglet-id',
                     'present': 'presence-coglets', 'limit': 'presence-limit', 'revision': 'uint64'})
    obj('presence-change', {'base': 'uint64', 'add': 'presence-coglets', 'remove': 'presence-coglets',
                           'attention': 'coglet-id', 'limit': 'presence-limit', 'reason': 'presence-reason'},
        optional=['attention', 'limit', 'reason'])
    enum('presence-urgency', ['low', 'high'])
    obj('presence-request', {'coglet': 'coglet-id', 'urgency': 'presence-urgency', 'reason': 'presence-reason'})
    # A cadence update is part of the same revision CAS, not an ungranted method.
    obj('presence-cadence', {'coglet': 'coglet-id', 'interval_ms': 'presence-cadence-ms',
                             'idle_interval_ms': 'presence-cadence-ms'})
    array('presence-cadences', 'presence-cadence', 255)
    obj('presence-get-params', {'cogent': 'presence-cogent-id'})
    obj('presence-set-params', {'cogent': 'presence-cogent-id', 'change': 'presence-change',
                               'cadences': 'presence-cadences'}, optional=['cadences'])
    obj('presence-request-params', {'cogent': 'presence-cogent-id', 'request': 'presence-request'})
    array('presence-held', 'hub-inbox-delivery', 64)
    obj('presence-check-in', {'coglet': 'coglet-id', 'status': 'hub-status',
                             'held': 'presence-held', 'dropped': 'uint64'})
    obj('presence-changed-params', {'presence': 'presence'})
    obj('presence-check-in-params', {'cogent': 'presence-cogent-id', 'revision': 'uint64',
                                    'check_in': 'presence-check-in'})


def requests(obj, enum):
    rules = []
    for method in ['get', 'set', 'request', 'changed', 'check_in']:
        name = 'presence-' + method.replace('_', '-') + '-request'
        fields = {'method': enum(name + '-method', ['presence.' + method]),
                  'target': 'hub-null' if method in ['get', 'set', 'request'] else 'coglet-id',
                  'correlation_id': 'correlation-id', 'timeout_ms': 'hub-timeout',
                  'params': 'presence-' + method.replace('_', '-') + '-params',
                  'context': 'hub-context'}
        obj(name, fields)
        rules.append(name)
    return rules


def fixtures(fixture, examples):
    import copy
    # Negative vectors run through every generated SDK and both references.
    for rule, path, bad in [
        ('presence-set-request', ('params', 'change', 'base'), -1),
        ('presence-set-request', ('params', 'change', 'limit'), 256),
        ('presence-set-request', ('params', 'change', 'add'), ['bad']),
        ('presence-request-request', ('params', 'request', 'urgency'), 'urgent'),
        ('presence-request-request', ('params', 'request', 'reason'), ''),
        ('presence-changed-request', ('params', 'presence', 'revision'), -1),
        ('presence-check-in-request', ('params', 'check_in', 'dropped'), -1),
        ('presence-check-in-request', ('params', 'check_in', 'status'), {}),
    ]:
        value = copy.deepcopy(examples[rule])
        parent = value
        for key in path[:-1]:
            parent = parent[key]
        parent[path[-1]] = bad
        for root in [rule, 'hub-request', 'hub-frame']:
            fixture(f'{rule}-bad-{"-".join(path)}-{root}', root, value, False)
    for rule in ['presence-get-request', 'presence-set-request', 'presence-request-request',
                 'presence-changed-request', 'presence-check-in-request']:
        value = copy.deepcopy(examples[rule])
        del value['context']
        fixture(rule + '-context-required-root', 'hub-frame', value, False)
    for code in ['conflict', 'invalid_argument', 'permission_denied', 'resource_exhausted']:
        value = copy.deepcopy(examples['hub-failure'])
        value['error'] = {'code': code, 'message': 'Presence refused', 'retryable': False}
        fixture('presence-refusal-' + code, 'hub-frame', value, True)
