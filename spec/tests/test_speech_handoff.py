"""Temporal oracle and real-harness case-contract tests; no device claims."""
import asyncio
import copy
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest

from test_common import encode
import test_surface_speech as surface_tests
import build_speech_handoff as build

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'conformance'))
import harness


class Oracle:
    """Small executable reading of ADR-0010, not a production work executor."""
    def __init__(self):
        self.epoch = 1
        self.seen = {}
        self.pending = {}
        self.done = []
        self.items = {}
        self.heard = {}
        self.dispatches = 0

    def apply(self, step):
        op = step['op']
        if op == 'item':
            assert step['item'] not in self.items
            ends = [s['end_ms'] for s in step['spans']]
            assert ends == sorted(ends) and all(0 <= x <= step['duration_ms'] for x in ends)
            self.items[step['item']] = dict(step, played_ms=0, stopped=False)
        elif op in ('position', 'barge'):
            item = self.items[step['item']]
            played = step['played_ms']
            assert item['played_ms'] <= played <= item['duration_ms']
            assert not item['stopped'] or played == item['played_ms']
            item['played_ms'] = played
            if op == 'barge':
                item['stopped'] = step['reflex'] == 'stopped'
                self.cancel()
        elif op == 'fence':
            self.cancel()
        elif op == 'heard':
            rec = step['record']; item = self.items[rec['item']]
            expected = ''.join(s['text'] for s in item['spans'] if s['end_ms'] <= item['played_ms'])
            assert rec == {'item':rec['item'], 'played_ms':item['played_ms'], 'text':expected}
            self.heard[rec['item']] = rec
        elif op in ('intent', 'result'):
            value = step['value']; rejection = None
            if op == 'intent':
                if value['epoch'] != self.epoch:
                    rejection = 'not_found'
                elif value['call'] in self.seen:
                    if self.seen[value['call']] != value:
                        rejection = 'conflict'
                elif len(encode(value['arguments'])) > 8192:
                    rejection = 'invalid_argument'
                elif len(self.pending) >= 4 or len(self.seen) >= 128:
                    rejection = 'resource_exhausted'
                else:
                    self.seen[value['call']] = copy.deepcopy(value)
                    self.pending[value['call']] = value
                    self.dispatches += 1
            else:
                pending = self.pending.get(value['call'])
                if value['epoch'] != self.epoch or pending is None:
                    rejection = 'not_found'
                elif value['turn'] != pending['turn']:
                    rejection = 'conflict'
                elif len(encode(value['value'])) > 32000:
                    rejection = 'invalid_argument'
                else:
                    assert value['status'] in ('succeeded', 'failed')
                    self.pending.pop(value['call'])
                    self.done.append(value)
            assert rejection == step.get('reject'), (step, rejection)
        elif op == 'claim':
            # Exact labelled summary evidence, not natural-language entailment.
            assert step['value'] in self.done and step['value']['status'] != 'cancelled'
        else:
            raise AssertionError('unknown trace op '+op)

    def cancel(self):
        assert self.epoch < 2**64-1
        self.epoch += 1  # fence BEFORE callbacks/cancellation notification
        pending = list(self.pending.values())
        self.pending.clear()
        self.seen.clear()
        for value in pending:
            self.done.append({k:value[k] for k in ('turn','call','epoch')} | {
                'status':'cancelled', 'value':dict(build.err, code='cancelled')})

    def state(self):
        return dict(heard=list(self.heard.values()), epoch=self.epoch,
                    dispatches=self.dispatches, done=self.done)


class SpeechHandoff(unittest.TestCase):
    def test_generated_current(self):
        for path, text in build.outputs().items():
            self.assertEqual((ROOT/path).read_text(), text, path)

    def test_every_golden_trace(self):
        traces = json.loads((ROOT/'spec/fixtures/speech/handoff.fixture').read_text())
        self.assertEqual(len(traces), 9)
        self.assertEqual(len({t['name'] for t in traces}), len(traces))
        for trace in traces:
            with self.subTest(trace=trace['name']):
                oracle = Oracle()
                if trace['valid']:
                    for step in trace['steps']: oracle.apply(step)
                    self.assertEqual(oracle.state(), trace['expected'])
                else:
                    with self.assertRaises(AssertionError):
                        for step in trace['steps']: oracle.apply(step)

    def test_budget_boundaries_and_pending_slots(self):
        for size, rejected in [(8186,False),(8187,True)]:
            # {"x": <text>} encodes to six overhead bytes at this length.
            value = dict(build.INTENT, arguments={'x':'é'*(size//2)+'a'*(size%2)})
            self.assertEqual(len(encode(value['arguments'])), size+6)
            step = {'op':'intent','value':value}
            if rejected: step['reject']='invalid_argument'
            Oracle().apply(step)
        oracle = Oracle()
        for i in range(5):
            step = {'op':'intent','value':dict(build.INTENT, call=f'{i:032x}')}
            if i == 4: step['reject']='resource_exhausted'
            oracle.apply(step)
        self.assertEqual(oracle.dispatches,4)
        for size, rejected in [(31994,False),(31995,True)]:
            oracle = Oracle(); oracle.apply({'op':'intent','value':build.INTENT})
            result = dict(build.RESULT, value={'x':'a'*size})
            self.assertEqual(len(encode(result['value'])), size+6)
            step = {'op':'result','value':result}
            if rejected: step['reject']='invalid_argument'
            oracle.apply(step)
            self.assertEqual(len(oracle.pending),int(rejected))

    def test_dedup_retains_completed_ids_and_fences_late_results(self):
        oracle = Oracle()
        for i in range(128):
            intent = dict(build.INTENT, call=f'{i:032x}')
            result = dict(build.RESULT, call=f'{i:032x}')
            oracle.apply({'op':'intent','value':intent})
            oracle.apply({'op':'result','value':result})
        oracle.apply({'op':'intent','value':dict(build.INTENT, call=f'{128:032x}'), 'reject':'resource_exhausted'})
        oracle.apply({'op':'intent','value':dict(build.INTENT, call=f'{0:032x}')})
        self.assertEqual(oracle.dispatches,128)
        for reason in ('barge-duck','close','revoke','disconnect','expiry','kill','mode','idle-user-start'):
            with self.subTest(reason=reason):
                oracle=Oracle(); oracle.apply({'op':'intent','value':build.INTENT})
                oracle.apply({'op':'fence','reason':reason})
                oracle.apply({'op':'result','value':build.RESULT,'reject':'not_found'})
                self.assertEqual(oracle.done,[build.CANCEL])

    def test_heard_multiple_items_and_duck(self):
        oracle = Oracle()
        oracle.apply({'op':'item', **build.ITEM})
        oracle.apply({'op':'barge','item':build.cid,'played_ms':1200,'reflex':'ducked'})
        oracle.apply({'op':'heard','record':{'item':build.cid,'played_ms':1200,'text':'Hello'}})
        oracle.apply({'op':'position','item':build.cid,'played_ms':3000})
        oracle.apply({'op':'heard','record':{'item':build.cid,'played_ms':3000,'text':'Hello world'}})
        oracle.apply({'op':'item', **dict(build.ITEM,item='6'*32,spans=[])})
        oracle.apply({'op':'heard','record':{'item':'6'*32,'played_ms':0,'text':''}})
        with self.assertRaises(AssertionError):
            oracle.apply({'op':'position','item':build.cid,'played_ms':1200})
        with self.assertRaises(AssertionError):
            oracle.apply({'op':'position','item':build.cid,'played_ms':3001})


class HandoffSchemas(surface_tests.ProfileFixtures):
    # Reuse actual JSON/reference-CDDL validator, without duplicating base tests.
    def test_new_rules_and_union_roots(self):
        for rule in ('heard-query','heard-record','work-state','work-intent',
                     'work-result','work-cancelled','work-done'):
            value = build.values['speech'][rule]
            for selected in (rule,'speech-profile'):
                self.assertTrue(self.validate('speech',selected,value))
                self.assertFalse(self.validate('speech',selected,dict(value,unexpected=True)))
        for status in ('failed','cancelled'):
            value = dict(build.RESULT,status=status,value=dict(build.err,code='cancelled'))
            self.assertTrue(self.validate('speech','work-done',value))
            self.assertEqual(self.validate('speech','work-result',value),status=='failed')
        for field, value in [('epoch',0),('epoch',True),('call','not-an-id')]:
            self.assertFalse(self.validate('speech','work-intent',dict(build.INTENT,**{field:value})))
        self.assertFalse(self.validate('speech','work-state',{'epoch':1,'pending':5}))


# Do not inherit the base fixture tests a second time in unittest discovery.
for _name in tuple(vars(surface_tests.ProfileFixtures)):
    if _name.startswith('test_'):
        setattr(HandoffSchemas, _name, None)


class Writer:
    def __init__(self): self.frames=[]
    def write(self, data): self.frames.append(data)
    async def drain(self): pass


class HandoffHarness(unittest.IsolatedAsyncioTestCase):
    async def run_case(self, case, corrupt=None):
        harness.validate_case(case)
        reader=asyncio.StreamReader()
        writer=Writer()
        peer=SimpleNamespace(impl={'transport':'stdio'},reader=reader,writer=writer)
        frames=[]
        # Expand predicate frames for a scripted reader only, not an implementation.
        for step in case['steps']:
            if 'expect' in step: frames.append(copy.deepcopy(step['expect']))
            elif 'match' in step:
                frame={}
                for path, value in step['match'].items():
                    at=frame; keys=path.split('.')
                    for key in keys[:-1]: at=at.setdefault(key,{})
                    at[keys[-1]]=copy.deepcopy(value)
                frames.append(frame)
            else: frames.append(None)
        if corrupt: corrupt(frames)
        async def feed():
            # Respect silence intervals; never feed EOF as proof of silence.
            # Give the consumer time to enter each read after its preceding sends.
            index=0
            for step in case['steps']:
                if 'expect' in step:
                    value=frames[index]; index+=1
                elif 'match' in step:
                    value=frames[index]; index+=1
                else:
                    index+=1
                    if 'silence_ms' in step:
                        await asyncio.sleep(step['silence_ms']/1000 + .015)
                    continue
                await asyncio.sleep(.005)
                raw=encode(value); reader.feed_data(len(raw).to_bytes(4,'big')+raw)
        task=asyncio.create_task(feed())
        try:
            await harness.timeline(peer,case)
            await task
            self.assertEqual(len(reader._buffer),0,'unconsumed frame')
            self.assertTrue(writer.frames)
        finally:
            task.cancel(); await asyncio.gather(task,return_exceptions=True)

    async def test_all_new_cases_pass_and_reject_corrupt_safety_fields(self):
        paths = [p for p in build.outputs() if p.startswith('conformance/')]
        self.assertEqual(len(paths),5)
        for path in paths:
            case=json.loads((ROOT/path).read_text())
            with self.subTest(case=case['name']):
                await self.run_case(case)
                def corrupt(frames):
                    if case['name'].startswith('mode-'):
                        frame=next(f for f in frames if f and f['type']=='capabilities')
                        frame['body']['entries'][0]['name']='speech.unknown'
                        return
                    # Mutate the last observable result, not handshake data.
                    frame=next(f for f in reversed(frames) if f is not None)
                    if frame['type']=='read_reply': frame['body']['value']['text']='unheard'
                    elif frame['type']=='grants': frame['body']['epoch']=99
                    elif frame['type']=='accept': frame['body']['accepted']=True
                    else: frame['body']['payload']['status']='failed'
                with self.assertRaises(AssertionError):
                    await self.run_case(case,corrupt)
