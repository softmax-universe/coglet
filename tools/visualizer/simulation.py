"""Offline M2 world re-execution. Never executes guests or dispatches effects.

This viewer envelope is not an ABI trace and does not authenticate its producer.
Guest output hashes remain recorded observations, not independently verified.
"""
import re

from coglet_sim.worlds import Gridbot, Cartpole

MAX_STEPS = 1000


def replay(archive, actions=None):
    if not isinstance(archive, dict) or archive.get('format') != 'coglet-world-replay-v1':
        raise ValueError('Expected coglet-world-replay-v1 envelope')
    world_name = archive.get('world')
    constructor = {'gridbot-v1': Gridbot, 'cartpole-v1': Cartpole}.get(world_name)
    if constructor is None:
        raise ValueError('Unsupported world')
    config = archive.get('config')
    allowed = {'width', 'height'} if world_name == 'gridbot-v1' else {'max_steps'}
    if not isinstance(config, dict) or set(config) != allowed:
        raise ValueError('Explicit world configuration required')
    recorded = archive.get('actions')
    if not isinstance(recorded, list) or not 1 <= len(recorded) <= MAX_STEPS:
        raise ValueError('Action count limit')
    choices = {'north', 'south', 'east', 'west'} if world_name == 'gridbot-v1' else {'left', 'right'}
    for sequence in (recorded, actions if actions is not None else recorded):
        if not isinstance(sequence, list) or len(sequence) != len(recorded):
            raise ValueError('Fork must preserve recorded action count')
        if any(not isinstance(a, str) or a not in choices for a in sequence):
            raise ValueError('Invalid world action')
    records = archive.get('records')
    if not isinstance(records, list) or not 1 <= len(records) <= MAX_STEPS * 4:
        raise ValueError('Recorded observation count limit')
    observations = {}
    identity = None
    previous_step = -1
    for record in records:
        if not isinstance(record, dict):
            raise ValueError('Invalid record')
        current = (record.get('coglet'), record.get('trace_id'))
        if identity is None:
            identity = current
        if current != identity:
            raise ValueError('Mixed trace identities')
        step = record.get('step')
        if type(step) is not int or step <= previous_step:
            raise ValueError('Record order')
        previous_step = step
        events = record.get('events')
        if not isinstance(events, list) or len(events) > 4096:
            raise ValueError('Invalid events')
        for event in events:
            if not isinstance(event, dict):
                raise ValueError('Invalid event')
            if event.get('kind') != 'sim.' + world_name.split('-')[0] + '_state':
                continue
            payload = event.get('payload')
            if not isinstance(payload, dict):
                raise ValueError('Invalid world observation')
            index, expected = payload.get('steps'), payload.get('state_hash')
            if type(index) is not int or not 0 <= index <= len(recorded):
                raise ValueError('Observation index')
            if not isinstance(expected, str) or not re.fullmatch('[0-9a-f]{64}', expected):
                raise ValueError('Invalid world hash')
            if index in observations:
                raise ValueError('Duplicate world observation')
            observations[index] = payload
    if set(observations) != set(range(len(recorded) + 1)):
        raise ValueError('Missing world observations')

    def execute(sequence):
        world = constructor(archive.get('seed'), **config)
        frames = []
        for index in range(len(sequence) + 1):
            state = world.snapshot()
            frames.append(dict(index=index, state=state, hash=world.state_hash(),
                               expected_hash=observations[index]['state_hash']))
            if index < len(sequence):
                world.step(sequence[index])
        return frames

    baseline = execute(recorded)
    for frame in baseline:
        payload = observations[frame['index']]
        if (frame['hash'] != frame['expected_hash'] or
                any(frame['state'].get(k) != v for k, v in payload.items() if k != 'state_hash')):
            raise ValueError('Recorded world replay mismatch')
    frames = execute(actions) if actions is not None else baseline
    divergence = next((f['index'] for f in frames if f['hash'] != f['expected_hash']), None)
    return dict(verification='world-model-only', world=world_name, frames=frames,
                actions=recorded if actions is None else actions, first_divergence=divergence)
