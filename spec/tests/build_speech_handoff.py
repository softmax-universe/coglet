"""ADR-0010 goldens and harness stimuli. No provider or audio implementation."""
import copy
import sys
from build_surface_fixtures import ROOT, values, cid, cog, err, event, action, dumps

CALL = '3' * 32
TURN = '4' * 32
STREAM = '2' * 32
INTENT = dict(values['speech']['work-intent'], call=CALL, turn=TURN)
RESULT = dict(values['speech']['work-result'], call=CALL, turn=TURN)
CANCEL = dict(RESULT, status='cancelled', value=dict(err, code='cancelled'))
ITEM = {'item':cid, 'duration_ms':3000,
        'spans':[{'end_ms':1000, 'text':'Hello'}, {'end_ms':3000, 'text':' world'}]}


def outputs():
    traces = []
    def trace(name, steps, expected, valid=True):
        traces.append(dict(name=name, valid=valid, steps=steps, expected=expected))
    heard = {'item':cid, 'played_ms':1200, 'text':'Hello'}
    playback = [{'op':'item', **ITEM}, {'op':'position', 'item':cid, 'played_ms':0},
                {'op':'barge', 'item':cid, 'played_ms':1200, 'reflex':'stopped'},
                {'op':'heard', 'record':heard}]
    trace('heard-1200-of-3000', playback, {'heard':[heard], 'epoch':2, 'dispatches':0, 'done':[]})
    wrong = copy.deepcopy(playback); wrong[-1]['record']['text'] = 'Hello world'
    trace('unheard-suffix-in-record', wrong, {}, False)
    wrong = copy.deepcopy(playback); wrong[-1]['record']['played_ms'] = 3000
    trace('synthesized-duration-is-not-played', wrong, {}, False)
    trace('work-cancelled-by-barge', [{'op':'intent','value':INTENT}, *playback,
          {'op':'result','value':RESULT,'reject':'not_found'}],
          {'heard':[heard], 'epoch':2, 'dispatches':1, 'done':[CANCEL]})
    trace('duplicate-call-ignored', [{'op':'intent','value':INTENT}, {'op':'intent','value':INTENT},
          {'op':'result','value':RESULT}, {'op':'claim','value':RESULT}],
          {'heard':[], 'epoch':1, 'dispatches':1, 'done':[RESULT]})
    trace('speak-before-result', [{'op':'intent','value':INTENT}, {'op':'claim','value':RESULT}], {}, False)
    bad = dict(RESULT, value={'summary':'Invented success'})
    trace('invented-outcome', [{'op':'intent','value':INTENT}, {'op':'result','value':RESULT},
          {'op':'claim','value':bad}], {}, False)
    trace('cancelled-work-cannot-speak-success', [{'op':'intent','value':INTENT}, *playback,
          {'op':'claim','value':RESULT}], {}, False)
    changed = dict(INTENT, arguments={'request':'Different'})
    trace('conflicting-call', [{'op':'intent','value':INTENT},
          {'op':'intent','value':changed,'reject':'conflict'}],
          {'heard':[], 'epoch':1, 'dispatches':1, 'done':[]})
    out = {'spec/fixtures/speech/handoff.fixture':dumps(traces)}

    def read(cap, args):
        return {'type':'read','body':dict(values['surface']['read'], capability=cap, args=args, deadline=5000000)}
    def reply(value):
        return {'type':'read_reply','body':{'correlation_id':cid, 'ok':True, 'value':value, 'snapshot':1}}
    def expect(msg):
        return {'expect':msg,'within_ms':200}
    def match(**pred):
        return {'match':pred,'within_ms':200}
    def case(name, steps):
        out['conformance/speech/'+name+'.json'] = dumps({
            'name':name, 'peer_role':'runtime', 'fixtures':['speech-handoff-'+name], 'steps':steps})
    # Each fixture starts after the normal hello/caps/grants setup below. It
    # reserves a quiet 5s action/grant window and scripts only the listed callbacks.
    hello = {'type':'hello','body':values['surface']['hello']}
    setup = [expect(hello), {'send':{'type':'hello_reply','body':values['surface']['hello-reply']}},
             match(type='capabilities', **{'body.revision':1}), match(type='grants', **{'body.epoch':1})]
    a = action('speech.speak_chunk', {'stream':STREAM,'chunk':0,'text':'Hello world','final':True})
    a['body']['action']['deadline'] = 5000000
    barge = {'utterance':TURN,'stream':STREAM,'item':cid,'played_ms':1200,'position_ms':1200,'reflex':'stopped'}
    pos = {'stream':STREAM,'chunk':0,'item':cid,'played_ms':1200,'position_ms':1200,'state':'stopped'}
    cancelled_action = {'id':cid,'correlation_id':cid,'status':'cancelled','error':dict(err,code='cancelled')}
    # seq/time are deliberately predicates only: unrelated initial setup need not
    # fix globally persistent sequence numbers. All safety fields are exact.
    def ev(kind, payload):
        return match(type='event', **{'body.kind':kind,'body.payload':payload})
    interruption = [ev('speech.start',{'utterance':TURN,'position_ms':0}),
                    ev('speech.barge_in',barge), ev('speech.position',pos)]
    case('heard-truncation', setup + [{'send':a}, match(type='accept', **{'body.id':cid,'body.accepted':True}),
         ev('speech.position',dict(pos,played_ms=0,position_ms=0,state='playing')),
         *interruption, ev('action.outcome',cancelled_action),
         {'send':read('speech.heard',{'item':cid})}, expect(reply(heard))])
    work_action = action('speech.work_result',RESULT,aid='5'*32)
    work_action['body']['action']['deadline'] = 5000000
    case('work-cancelled', setup + [ev('speech.work_intent',INTENT),
         *interruption, ev('speech.work_done',CANCEL),
         {'send':read('speech.work_state',{})}, expect(reply({'epoch':2,'pending':0})),
         {'send':work_action}, match(type='accept', **{'body.id':'5'*32,'body.accepted':False,'body.error.code':'not_found'}),
         {'silence_ms':25}])
    case('duplicate-call', setup + [ev('speech.work_intent',INTENT), {'silence_ms':25},
         {'send':read('speech.work_state',{})}, expect(reply({'epoch':1,'pending':1})),
         {'send':work_action}, match(type='accept', **{'body.id':'5'*32,'body.accepted':True}),
         ev('speech.work_done',RESULT),
         match(type='event', **{'body.kind':'action.outcome','body.payload.id':'5'*32,'body.payload.status':'succeeded'}),
         {'silence_ms':25}])
    for mode in ('hands_free','push_to_talk'):
        marker = {'name':'speech.'+mode,'events':[],'reads':[],'actions':[],'exclusive':[],'schemas':{}}
        # Mode-only fixture advertises this one entry; it does not activate capture.
        case('mode-'+mode.replace('_','-'), [expect(hello), setup[1],
             match(type='capabilities', **{'body.entries':[marker]}), setup[3]])
    return out


def main():
    for path, text in outputs().items():
        target = ROOT/path
        if '--check' in sys.argv:
            if target.read_text() != text:
                raise SystemExit('stale '+path)
        else:
            target.parent.mkdir(parents=True,exist_ok=True)
            target.write_text(text)
    print('speech handoff goldens and cases: OK')


if __name__ == '__main__':
    main()
