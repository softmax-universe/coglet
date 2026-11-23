"""Rebuild surface/speech/expression schemas and goldens from the profile tables.

Run with --check in CI: generated files must exactly match these definitions.
Common definitions are imported, never copied. No runtime implementation here.
"""
import argparse
import copy
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).parent))
from test_common import encode

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / 'spec'
BASE = 'https://coglet.dev/spec/messages/'
COMMON = {'uint64', 'correlation-id', 'coglet-id', 'surface-id', 'capability-name',
          'method-name', 'version-offer', 'version-selection', 'event', 'action',
          'action-accept', 'error', 'profile-map'}


def ref(name):
    return {'$ref': ('common.schema.json#/$defs/' if name in COMMON else '#/$defs/') + name}


def record(**fields):
    return {'type': 'object', 'properties': fields, 'required': list(fields),
            'additionalProperties': False}


def enum(*values):
    return {'enum': list(values)}


def integer(lo, hi):
    return {'type': 'integer', 'minimum': lo, 'maximum': hi}


def array(item):
    return {'type': 'array', 'items': item}


def text(lo=0, hi=16384):
    return {'type': 'string', 'minLength': lo, 'maxLength': hi}


def alternatives(*items):
    return {'anyOf': list(items)}


def cddl(schema):
    if '$ref' in schema:
        return schema['$ref'].split('/')[-1]
    if 'enum' in schema:
        return ' / '.join(json.dumps(x) for x in schema['enum'])
    if 'anyOf' in schema:
        return ' / '.join(cddl(x) for x in schema['anyOf'])
    kind = schema['type']
    if kind == 'object':
        if 'properties' not in schema:
            return '{* (' + cddl(schema['propertyNames']) + ') => (' + cddl(schema['additionalProperties']) + ')}'
        return '{' + ', '.join(json.dumps(k) + ': (' + cddl(v) + ')' for k, v in schema['properties'].items()) + '}'
    if kind == 'array':
        return '[* (' + cddl(schema['items']) + ')]'
    if kind == 'integer':
        return f"{schema['minimum']}..{schema['maximum']}"
    if kind == 'string':
        # JSON maxLength and this regexp count Unicode scalar values, not UTF-8 bytes.
        pattern = '^[\\s\\S]{' + str(schema['minLength']) + ',' + str(schema['maxLength']) + '}$'
        return 'tstr .regexp ' + json.dumps(pattern)
    if kind == 'boolean':
        return 'bool'
    raise ValueError(schema)


cid = '1' * 32
sid = 'surface:' + 'a' * 64
cog = 'coglet:' + 'b' * 64
u = ref('uint64')
rid = ref('correlation-id')
boolean = {'type': 'boolean'}
err = {'code': 'permission_denied', 'message': 'No grant', 'retryable': False}
exprs = enum('neutral', 'listening', 'thinking', 'speaking', 'happy', 'concerned')

surface = {
    'hello': record(surface=ref('surface-id'), offer=ref('version-offer'), clock=u, correlation_id=rid),
    'hello-reply': record(selection=ref('version-selection'), correlation_id=rid),
    'schema-refs': {'type':'object','propertyNames':ref('capability-name'),'additionalProperties':text(1,2048)},
    'cap-entry': record(name=ref('capability-name'), events=array(ref('capability-name')),
                        reads=array(ref('capability-name')), actions=array(ref('capability-name')),
                        exclusive=array(ref('capability-name')), schemas=ref('schema-refs')),
    'capabilities': record(revision=integer(1, 2**64-1), clock=u, entries=array(ref('cap-entry')), correlation_id=rid),
    'grant': record(coglet=ref('coglet-id'), operations=array(ref('capability-name')),
                    resources=array(ref('capability-name')), expires=u),
    'grants': record(epoch=integer(1, 2**64-1), entries=array(ref('grant')), correlation_id=rid),
    'read': record(coglet=ref('coglet-id'), capability=ref('capability-name'), args=ref('profile-map'),
                   grant_epoch=integer(1, 2**64-1), deadline=u, correlation_id=rid),
    'read-reply': alternatives(record(correlation_id=rid, ok=enum(True), value=ref('profile-map'), snapshot=u),
                               record(correlation_id=rid, ok=enum(False), error=ref('error'))),
    'act': record(grant_epoch=integer(1, 2**64-1), action=ref('action')),
    'health': record(state=enum('ready', 'degraded', 'stopped'), clock=u, pending=u, correlation_id=rid),
    'kill': record(reason=text(1), correlation_id=rid),
    'surface-error': record(error=ref('error'), correlation_id=rid),
}
wire = {'hello': 'hello', 'hello_reply': 'hello-reply', 'capabilities': 'capabilities',
        'grants': 'grants', 'event': 'event', 'read': 'read', 'read_reply': 'read-reply',
        'act': 'act', 'accept': 'action-accept', 'health': 'health', 'kill': 'kill', 'error': 'surface-error'}
surface['surface-message'] = alternatives(*(record(type=enum(k), body=ref(v)) for k, v in wire.items()))

speech = {
    'speech-start': record(utterance=rid, position_ms=u),
    'speech-partial': record(utterance=rid, revision=integer(1, 2**64-1), text=text()),
    'speech-end': record(utterance=rid, revision=integer(1, 2**64-1), text=text(), position_ms=u),
    'speech-position': record(stream=rid, chunk=u, item=rid, played_ms=u, position_ms=u, state=enum('playing','ducked','stopped','finished')),
    'speech-barge-in': record(utterance=rid, stream=rid, item=rid, played_ms=u, position_ms=u, reflex=enum('stopped','ducked')),
    'speak-chunk': record(stream=rid, chunk=u, text=text(1,4096), final=boolean),
    'stop': record(stream=rid),
    'backchannel': record(stream=rid, cue=enum('ack','thinking','agree')),
    'speech-route': record(placement=enum('surface','runtime','hub','provider'), route=text(1,256)),
    'speech-config': record(stt=ref('speech-route'), tts=ref('speech-route'), barge=enum('stop','duck'),
                            session_mode=enum('text','speech_to_speech')),
    'session-open': record(session=rid, route=text(1,256)),
    'session-close': record(session=rid),
    'tool-call': record(session=rid, call=rid, name=ref('method-name'), arguments=ref('profile-map')),
    'tool-result': alternatives(record(session=rid, call=rid, ok=enum(True), value=ref('profile-map')),
                                record(session=rid, call=rid, ok=enum(False), value=ref('error'))),
}
# ADR-0010: additive records; existing 0.1 shapes remain unchanged.
speech.update({
    'heard-query': record(item=rid),
    'heard-record': record(item=rid, played_ms=u, text=text()),
    'work-state': record(epoch=integer(1, 2**64-1), pending=integer(0, 4)),
    'work-intent': record(turn=rid, call=rid, epoch=integer(1, 2**64-1),
                          name=ref('method-name'), arguments=ref('profile-map')),
    'work-result': alternatives(
        record(turn=rid, call=rid, epoch=integer(1, 2**64-1), status=enum('succeeded'), value=ref('profile-map')),
        record(turn=rid, call=rid, epoch=integer(1, 2**64-1), status=enum('failed'), value=ref('error'))),
    'work-cancelled': record(turn=rid, call=rid, epoch=integer(1, 2**64-1), status=enum('cancelled'), value=ref('error')),
    'work-done': alternatives(ref('work-result'), ref('work-cancelled')),
})
expression = {
    'set-expression': record(expression=exprs, intensity=integer(0,1000), duration_ms=integer(0,60000)),
    'expression-state': record(expression=exprs, intensity=integer(0,1000), remaining_ms=integer(0,60000)),
}


def event(kind, payload, seq=0, clock=0):
    return {'type':'event','body':{'source':sid,'kind':kind,'seq':seq,'logical_time':clock,
                                   'payload':payload,'correlation_id':cid}}


def action(capability, args, aid=cid):
    return {'type':'act','body':{'grant_epoch':1,'action':{'id':aid,'coglet':cog,
        'capability':capability,'args':args,'deadline':1000000,'correlation_id':aid}}}


values = {
 'surface': {
    'hello': {'surface':sid,'offer':{'versions':['0.1']},'clock':0,'correlation_id':cid},
    'hello-reply': {'selection':{'version':'0.1'},'correlation_id':cid},
    'cap-entry': {'name':'speech.control','events':['speech.start'],'reads':['speech.config'],
                  'actions':['speech.stop'],'exclusive':['audio.mic','audio.speaker'],
                  'schemas':{'speech.start':BASE+'speech.schema.json#/$defs/speech-start',
                             'speech.config':BASE+'speech.schema.json#/$defs/speech-config',
                             'speech.stop':BASE+'speech.schema.json#/$defs/stop'}},
    'grant': {'coglet':cog,'operations':['speech.stop'],'resources':['audio.speaker'],'expires':1000000},
    'read': {'coglet':cog,'capability':'speech.config','args':{},'grant_epoch':1,'deadline':1000000,'correlation_id':cid},
    'read-reply': {'correlation_id':cid,'ok':True,'value':{},'snapshot':1},
    'act': action('speech.stop',{'stream':cid})['body'],
    'health': {'state':'ready','clock':0,'pending':0,'correlation_id':cid},
    'kill': {'reason':'operator','correlation_id':cid},
    'surface-error': {'error':err,'correlation_id':cid},
 },
 'speech': {
    'speech-start': {'utterance':cid,'position_ms':0},
    'speech-partial': {'utterance':cid,'revision':1,'text':'hello'},
    'speech-end': {'utterance':cid,'revision':2,'text':'hello world','position_ms':500},
    'speech-position': {'stream':cid,'chunk':0,'item':cid,'played_ms':30,'position_ms':30,'state':'playing'},
    'speech-barge-in': {'utterance':cid,'stream':cid,'item':cid,'played_ms':30,'position_ms':30,'reflex':'stopped'},
    'speak-chunk': {'stream':cid,'chunk':0,'text':'Hello','final':True},
    'stop': {'stream':cid}, 'backchannel':{'stream':cid,'cue':'ack'},
    'speech-route': {'placement':'surface','route':'local'},
    'speech-config': {'stt':{'placement':'surface','route':'local'},'tts':{'placement':'hub','route':'voice'},
                      'barge':'stop','session_mode':'text'},
    'session-open': {'session':cid,'route':'configured_provider'},
    'session-close': {'session':cid},
    'tool-call': {'session':cid,'call':cid,'name':'game.move','arguments':{'x':1}},
    'tool-result': {'session':cid,'call':cid,'ok':True,'value':{}},
 },
 'expression': {'set-expression': {'expression':'listening','intensity':500,'duration_ms':1000},
                'expression-state': {'expression':'listening','intensity':500,'remaining_ms':900}},
}
values['speech'].update({
    'heard-query': {'item':cid},
    'heard-record': {'item':cid, 'played_ms':1200, 'text':'Hello'},
    'work-state': {'epoch':1, 'pending':0},
    'work-intent': {'turn':cid, 'call':cid, 'epoch':1, 'name':'game.inspect', 'arguments':{'request':'Inspect'}},
    'work-result': {'turn':cid, 'call':cid, 'epoch':1, 'status':'succeeded', 'value':{'summary':'Inspected'}},
    'work-cancelled': {'turn':cid, 'call':cid, 'epoch':1, 'status':'cancelled', 'value':dict(err, code='cancelled')},
    'work-done': {'turn':cid, 'call':cid, 'epoch':1, 'status':'succeeded', 'value':{'summary':'Inspected'}},
})
values['surface']['capabilities'] = {'revision':1,'clock':0,'entries':[values['surface']['cap-entry']],'correlation_id':cid}
values['surface']['grants'] = {'epoch':1,'entries':[values['surface']['grant']],'correlation_id':cid}
values['surface']['surface-message'] = {'type':'hello','body':values['surface']['hello']}


def dumps(value):
    return json.dumps(value, indent=2, ensure_ascii=False) + '\n'


def outputs():
    result = {}
    for name, definitions in [('surface',surface),('speech',speech),('expression',expression)]:
        entry = 'surface-message' if name == 'surface' else name + '-profile'
        if name != 'surface':
            definitions = dict(definitions, **{entry: alternatives(*(ref(k) for k in definitions))})
        schema = {'$schema':'https://json-schema.org/draft/2020-12/schema', '$id':BASE+name+'.schema.json',
                  '$ref':'#/$defs/'+entry, '$defs':definitions}
        result[f'spec/messages/{name}.schema.json'] = dumps(schema)
        result[f'spec/messages/{name}.cddl'] = '; Generated by spec/tests/build_surface_fixtures.py; import common.cddl.\n' + \
            name + '-entry = ' + entry + '\n' + '\n'.join(k+' = '+cddl(v) for k,v in definitions.items())+'\n'
        cases=[]
        for rule,value in values[name].items():
            cases.append({'name':rule+'-valid','type':rule,'valid':True,'value':value,'cbor_hex':encode(value).hex()})
            bad=copy.deepcopy(value); bad['unexpected']=True
            cases.append({'name':rule+'-unknown-field','type':rule,'valid':False,'value':bad})
            bad=copy.deepcopy(value); del bad[next(iter(bad))]
            cases.append({'name':rule+'-missing-field','type':rule,'valid':False,'value':bad})
        # Exercise each wire discriminator, not only the hello branch.
        if name == 'surface':
            for typ,rule in wire.items():
                value = values[name].get(rule)
                if typ=='event': value=event('speech.start',values['speech']['speech-start'])['body']
                if typ=='accept': value={'id':cid,'correlation_id':cid,'accepted':True}
                msg={'type':typ,'body':value}
                cases.append({'name':'wire-'+typ,'type':'surface-message','valid':True,'value':msg,'cbor_hex':encode(msg).hex()})
            for value in [{'correlation_id':cid,'ok':False,'error':err},
                          {'correlation_id':cid,'ok':True,'value':{},'snapshot':0,'error':err}]:
                valid = 'snapshot' not in value
                cases.append({'name':'read-reply-error-'+str(valid),'type':'read-reply','valid':valid,'value':value,
                              **({'cbor_hex':encode(value).hex()} if valid else {})})
        if name == 'speech':
            value={'session':cid,'call':cid,'ok':False,'value':err}
            cases.append({'name':'tool-result-error','type':'tool-result','valid':True,'value':value,'cbor_hex':encode(value).hex()})
            for rule,field,badval in [('speech-partial','revision',0),('speak-chunk','text',''),
                                     ('speech-route','placement','cloud'),('backchannel','cue','product_foo')]:
                value=dict(values[name][rule], **{field:badval})
                cases.append({'name':rule+'-bad-value','type':rule,'valid':False,'value':value})
        if name == 'expression':
            for field,val in [('intensity',1001),('duration_ms',60001),('expression','mac_only')]:
                cases.append({'name':'expression-bad-'+field,'type':'set-expression','valid':False,
                              'value':dict(values[name]['set-expression'],**{field:val})})
        result[f'spec/fixtures/{name}/cases.json']=dumps(cases)
    return result


def main():
    check = '--check' in sys.argv
    failed=[]
    for path,content in outputs().items():
        target=ROOT/path
        if check:
            if not target.exists() or target.read_text()!=content: failed.append(path)
        else:
            target.parent.mkdir(parents=True,exist_ok=True); target.write_text(content)
    if failed:
        raise SystemExit('Stale generated files: '+', '.join(failed))
    print('surface/speech/expression schema and fixture generation: OK')


if __name__=='__main__': main()
