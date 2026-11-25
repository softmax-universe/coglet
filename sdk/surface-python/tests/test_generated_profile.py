"""Generated surface02 selection must not mutate or widen frozen roots."""
from copy import deepcopy

import pytest

from coglet_surface.catalog import wire_schema
from coglet_surface.codec import validate
from coglet_surface.generated import SCHEMA


def test_generated_surface02_selection_and_frozen_identity():
    before = deepcopy(SCHEMA)
    strict = wire_schema('0.1')
    selected = wire_schema('0.2')
    for family, rules in (
            ('common', ('capability-name', 'event', 'action')),
            ('surface', ('surface-message', 'cap-entry', 'capabilities',
                         'grant', 'grants', 'read', 'act', 'schema-refs'))):
        for rule in rules:
            assert selected[f'{family}::{rule}'] == {
                'op': 'ref', 'name': f'surface02::s02-{rule}'}
            assert strict[f'{family}::{rule}'] == SCHEMA[f'{family}::{rule}']
    validate('common::capability-name', 'softmax.app.state', selected)
    with pytest.raises(ValueError):
        validate('common::capability-name', 'softmax.app.state', strict)
    for invalid in ('softmax.app.state.extra', 'other.app.state',
                    'softmax.app.state\n', 'Softmax.app.state'):
        with pytest.raises(ValueError):
            validate('common::capability-name', invalid, selected)
    changed = {key for key in SCHEMA if strict[key] != selected[key]}
    assert changed == {f'{family}::{rule}' for family, rules in (
        ('common', ('capability-name', 'event', 'action')),
        ('surface', ('surface-message', 'cap-entry', 'capabilities', 'grant',
                     'grants', 'read', 'act', 'schema-refs'))) for rule in rules}
    # Full generated message routing, not just the leaf-name grammar.
    from test_surface import COGLET, CID
    message = dict(type='read', body=dict(
        coglet=COGLET, capability='softmax.app.state', args={},
        grant_epoch=1, deadline=100, correlation_id=CID))
    validate('surface::surface-message', message, selected)
    with pytest.raises(ValueError):
        validate('surface::surface-message', message, strict)
    assert SCHEMA == before
