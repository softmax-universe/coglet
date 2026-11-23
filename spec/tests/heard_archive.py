"""M-C journal contract checker/replay; no native or persistence qualification."""
import json
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from test_common import SPEC, encode, decode

HEARD_PATH = '/data/runtime/heard.cbor'


def read_state(raw):
    if raw is None:
        return {'items': []}
    try:
        data = bytes.fromhex(raw)
        state = decode(data)
        if encode(state) != data:
            raise ValueError('noncanonical bytes')
        abi = json.loads((SPEC / 'messages/abi.schema.json').read_text())
        common = json.loads((SPEC / 'messages/common.schema.json').read_text())
        registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in (abi, common))
        if not Draft202012Validator({'$ref': abi['$id']+'#/$defs/heard-state'}, registry=registry).is_valid(state):
            raise ValueError('shape')
        keys = [(i['surface'], i['item']) for i in state['items']]
        if keys != sorted(set(keys)):
            raise ValueError('item order or duplicate')
        for item in state['items']:
            ends = [s['end_ms'] for s in item['spans']]
            if ends != sorted(set(ends)) or any(t > item['duration_ms'] for t in ends):
                raise ValueError('alignment ends')
            if item['played_ms'] > item['duration_ms']:
                raise ValueError('played past duration')
            prefix = ''.join(s['text'] for s in item['spans'] if s['end_ms'] <= item['played_ms'])
            prefix = prefix.encode('utf-8')[:16384].decode('utf-8', errors='ignore')
            if prefix != item['heard']:
                raise ValueError('prefix')
        return state
    except (ValueError, TypeError, KeyError) as error:
        raise ValueError('heard state: ' + str(error)) from error


def check_transition(old_raw, new_raw):
    if new_raw is None:
        raise ValueError('heard deletion')
    old, new = read_state(old_raw), read_state(new_raw)
    after = {(i['surface'], i['item']): i for i in new['items']}
    for item in old['items']:
        current = after.get((item['surface'], item['item']))
        if current is None:
            raise ValueError('heard item removed')
        if (current['duration_ms'] < item['duration_ms'] or
                current['spans'][:len(item['spans'])] != item['spans']):
            raise ValueError('heard alignment changed')
        if current['played_ms'] < item['played_ms']:
            raise ValueError('heard position regressed')
        if item['stopped'] and current != item:
            raise ValueError('heard stopped item advanced')


def collect_owner_read(old_raw, surface, read, *, stopped=False):
    """Live collection contract oracle. Caller has authenticated the owner read.

    Stop is supplied by the existing playback boundary, not a new read field.
    Offline replay deliberately does not call this function.
    """
    state = read_state(old_raw)
    items = {(i['surface'], i['item']): i for i in state['items']}
    key = (surface, read['item'])
    item: dict | None = items.get(key)
    if item is not None and item['stopped']:
        return old_raw
    if item is None:
        item = dict(surface=surface, item=read['item'], duration_ms=0,
                    played_ms=0, stopped=False, spans=[], heard='')
        items[key] = item
    played = read['played_ms']
    if played < item['played_ms']:
        raise ValueError('heard position regressed')
    item['duration_ms'] = max(item['duration_ms'], played)
    archived = ''.join(s['text'] for s in item['spans']).encode('utf-8')
    text = read['text'].encode('utf-8')
    last_end = item['spans'][-1]['end_ms'] if item['spans'] else 0
    if played > last_end and text.startswith(archived) and len(text) > len(archived):
        item['spans'].append(dict(end_ms=played, text=text[len(archived):].decode('utf-8')))
    item.update(played_ms=played, stopped=stopped)
    prefix = ''.join(s['text'] for s in item['spans'] if s['end_ms'] <= played)
    item['heard'] = prefix.encode('utf-8')[:16384].decode('utf-8', errors='ignore')
    state['items'] = [items[k] for k in sorted(items)]
    new_raw = encode(state).hex()
    check_transition(old_raw, new_raw)
    return new_raw


def check_owner_read_archive(old_raw, new_raw, surface, read, *, stopped=False):
    """Live producer check, separate from archive-only replay validation."""
    check_transition(old_raw, new_raw)
    if collect_owner_read(old_raw, surface, read, stopped=stopped) != new_raw:
        raise ValueError('heard read mismatch')


def replay_heard(trace):
    # Only archive bytes are inputs. check_trace validates all boundaries/hashes;
    # never accept a projection or live oracle as a replay argument.
    from test_abi import check_trace
    check_trace(trace)
    files = dict(trace['initial_files'])
    states = [read_state(files.get(HEARD_PATH))]
    records = trace['records'] + ([trace['trailing_boundary']] if trace['trailing_boundary'] else [])
    for record in records:
        for change in record['boundary']:
            if change['method'] == 'runtime.heard':
                states.append(read_state(change['after']))
        files = record['output_files']
    projection = [{'surface': i['surface'], 'item': i['item'], 'played_ms': i['played_ms'], 'text': i['heard']}
                  for i in read_state(files.get(HEARD_PATH))['items']]
    return projection, states
