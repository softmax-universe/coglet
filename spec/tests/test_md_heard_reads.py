"""M-D owner-read -> archive contract; not native/runtime qualification."""
import copy
import json
import subprocess
import sys
import unittest

from heard_archive import HEARD_PATH, collect_owner_read, check_owner_read_archive, replay_heard
from build_mc_contracts import heard_vectors
from test_abi import digest, record_hash
from test_common import SPEC, encode, decode
import test_mc_contracts as mc


def vector():
    surface = 'surface:' + '2' * 64
    item = '1' * 32
    # Exact heard-record wire shape. Stop is a separate playback boundary input.
    reads = [dict(item=item, played_ms=p, text=t) for p, t in
             [(0, ''), (100, 'Hé'), (200, 'Héllo'), (250, 'revision'),
              (270, 'Héllo'), (280, ''), (300, 'Héllo!')]]
    states = []
    raw = None
    for n, read in enumerate(reads):
        raw = collect_owner_read(raw, surface, read, stopped=n == len(reads)-1)
        states.append(decode(bytes.fromhex(raw)))
    trace = copy.deepcopy(heard_vectors()[0]['value'])
    trace['initial_files'].pop(HEARD_PATH, None)
    trace['initial_hash'] = digest(trace['initial_files'])
    rec = trace['trailing_boundary']
    files = dict(trace['initial_files'])
    rec['boundary'] = []
    for n, state in enumerate(states):
        after = encode(state).hex()
        rec['boundary'].append(dict(actor=trace['coglet'], target=trace['coglet'], ticket=None,
                                    method='runtime.heard', audit_id=f'{n+1:032x}', path=HEARD_PATH,
                                    before=files.get(HEARD_PATH), after=after))
        files[HEARD_PATH] = after
    rec.update(before_hash=trace['initial_hash'], output_files=files, output_hash=digest(files))
    rec['record_hash'] = record_hash(rec)
    trace['final_hash'] = rec['output_hash']
    return surface, reads, states, trace


class MDHeardReads(unittest.IsolatedAsyncioTestCase):
    validate_wire = mc.MCContracts.validate_wire
    frames_through_harness = mc.MCContracts.frames_through_harness
    async def test_owner_reads_wire_archive_fresh_replay(self):
        surface, reads, states, trace = vector()
        for read in reads:
            self.validate_wire('speech', 'heard-record', read)
        self.assertEqual(states[-1]['items'][0]['spans'], [
            dict(end_ms=100, text='Hé'), dict(end_ms=200, text='llo'), dict(end_ms=300, text='!')])
        self.assertEqual([s['items'][0]['heard'] for s in states],
                         ['', 'Hé', 'Héllo', 'Héllo', 'Héllo', 'Héllo', 'Héllo!'])
        self.assertEqual([s['items'][0]['duration_ms'] for s in states], [0,100,200,250,270,280,300])
        self.assertEqual([s['items'][0]['played_ms'] for s in states], [0,100,200,250,270,280,300])
        for old, read, state in zip([None]+states[:-1], reads, states, strict=True):
            check_owner_read_archive(None if old is None else encode(old).hex(), encode(state).hex(),
                                     surface, read, stopped=state['items'][0]['stopped'])
        history, rebuilt = replay_heard(decode(encode(trace)))
        self.assertEqual(rebuilt[1:], states)
        self.assertEqual(history[0]['text'], 'Héllo!')
        # A genuinely fresh process receives archive bytes ONLY, not reads/state.
        result = subprocess.run([sys.executable, '-c',
            'import json,sys; from heard_archive import replay_heard; from test_common import decode; '
            'print(json.dumps(replay_heard(decode(bytes.fromhex(sys.stdin.read())))))'],
            input=encode(trace).hex(), text=True, capture_output=True, check=True,
            env={**__import__('os').environ, 'PYTHONPATH':str(SPEC/'tests')})
        self.assertEqual(json.loads(result.stdout), [history, rebuilt])
        case = dict(name='owner-reads-wire-archive-fresh-replay', steps=[
            dict(send=dict(operation='abi.collect_owner_reads', surface=surface, reads=reads,
                           stop_after=len(reads), archive=trace)),
            dict(match={'valid':True, 'history':history}, within_ms=1000)])
        await self.frames_through_harness(case, [dict(valid=True, history=history)])

    async def test_owner_read_archive_mismatch_and_nonappend_rejected(self):
        surface, reads, states, _ = vector()
        negatives = []
        bad = copy.deepcopy(states[1])
        bad['items'][0].update(spans=[dict(end_ms=100,text='forged')], heard='forged')
        negatives.append(('owner-read-archive-mismatch', states[0], bad, reads[1], 'heard read mismatch'))
        bad = copy.deepcopy(states[2])
        bad['items'][0]['spans'][0]['text'] = 'hé'
        bad['items'][0]['heard'] = 'héllo'
        negatives.append(('owner-read-archive-nonappend', states[1], bad, reads[2], 'heard alignment changed'))
        for name, old, bad, read, error in negatives:
            with self.assertRaisesRegex(ValueError, error):
                check_owner_read_archive(encode(old).hex(), encode(bad).hex(), surface, read)
            case = dict(name=name, steps=[
                dict(send=dict(operation='abi.check_owner_read_archive', surface=surface, read=read,
                               before=encode(old).hex(), after=encode(bad).hex())),
                dict(match={'valid':False}, within_ms=1000)])
            await self.frames_through_harness(case, [dict(valid=False)])

    def test_owner_reads_zero_empty_nonextension_regression_and_stop(self):
        surface, reads, states, _ = vector()
        raw = encode(states[-1]).hex()
        self.assertEqual(collect_owner_read(raw, surface, dict(reads[-1], played_ms=400,text='Héllo!!')), raw)
        bad = copy.deepcopy(states[-1])
        bad['items'][0]['duration_ms'] = 400
        bad['items'][0]['spans'].append(dict(end_ms=400, text='!'))
        from heard_archive import check_transition
        with self.assertRaisesRegex(ValueError, 'heard stopped item advanced'):
            check_transition(raw, encode(bad).hex())
        zero = decode(bytes.fromhex(collect_owner_read(None, surface, dict(reads[0], text='not heard'))))['items'][0]
        self.assertEqual((zero['spans'], zero['heard'], zero['played_ms'], zero['duration_ms']), ([], '', 0, 0))
        raw = encode(states[1]).hex()
        with self.assertRaisesRegex(ValueError, 'heard position regressed'):
            collect_owner_read(raw, surface, dict(reads[1],played_ms=99))
        for text in ['Hé', 'H', 'He\u0301llo', 'revision', '']:
            new = decode(bytes.fromhex(collect_owner_read(raw,surface,dict(reads[1],played_ms=150,text=text))))
            self.assertEqual(new['items'][0]['spans'],states[1]['items'][0]['spans'])
            self.assertEqual((new['items'][0]['played_ms'], new['items'][0]['duration_ms']), (150, 150))
        # Extension at the same end is not new alignment evidence.
        new = decode(bytes.fromhex(collect_owner_read(raw,surface,dict(reads[1],text='Héllo'))))
        self.assertEqual(new,states[1])
