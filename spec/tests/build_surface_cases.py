"""Golden surface framing and language-neutral wire scripts.

These scripts require the documented deterministic surface fixture configuration;
this generator does not impersonate an implementation under conformance test.
"""
import copy
import json
from pathlib import Path
import struct
import sys
from build_surface_fixtures import (ROOT, SPEC, cid, sid, cog, err, event, action, values, encode, dumps)


def outputs():
    out={}
    hello={'type':'hello','body':values['surface']['hello']}
    raw=encode(hello)
    frame=struct.pack('!I',len(raw))+raw
    vectors=[{'name':'valid-hello','hex':frame.hex(),'valid':True},
             {'name':'zero','hex':'00000000','valid':False},
             {'name':'oversize','hex':'00100001','valid':False},
             {'name':'short-header','hex':'0000','valid':False},
             {'name':'truncated-payload','hex':frame[:-1].hex(),'valid':False},
             {'name':'trailing-item','hex':(struct.pack('!I',len(raw)+1)+raw+b'\x00').hex(),'valid':False},
             {'name':'non-profile-bytes','hex':'000000024100','valid':False}]
    out['spec/fixtures/surface/framing.json']=dumps(vectors)
    cfg=values['speech']['speech-config']
    ops=['speech.start','speech.partial','speech.end','speech.position','speech.barge_in',
         'speech.tool_call','speech.config','speech.speak_chunk','speech.stop','speech.backchannel',
         'speech.session_open','speech.session_close','speech.tool_result',
         'expression.set_expression','expression.state']
    resources=['audio.mic','audio.speaker','display.face']
    def setup():
        return [{'expect':hello,'within_ms':200},
                {'send':{'type':'hello_reply','body':values['surface']['hello-reply']}},
                {'match':{'type':'capabilities','body.revision':1},'within_ms':200},
                {'match':{'type':'grants','body.epoch':1},'within_ms':200}]
    def accept(a,accepted=True,code=None):
        match={'type':'accept','body.id':a['body']['action']['id'],
               'body.correlation_id':a['body']['action']['correlation_id'],'body.accepted':accepted}
        if code: match['body.error.code']=code
        return {'match':match,'within_ms':200}
    def outcome(a,status):
        return {'match':{'type':'event','body.kind':'action.outcome',
                         'body.payload.id':a['body']['action']['id'],
                         'body.payload.correlation_id':a['body']['action']['correlation_id'],
                         'body.payload.status':status},'within_ms':200}
    def case(suite,name,steps):
        labels=['surface-base']
        if suite=='speech': labels.append('audio-'+name)
        if name=='kill-latched': labels.append('kill-at-100ms')
        out[f'conformance/{suite}/{name}.json']=dumps({'name':name,'peer_role':'runtime','fixtures':labels,'steps':setup()+steps})
    # The normative fixture setup is separate from wire scripts; not an extension
    # of the harness's {name,steps} envelope.
    out['spec/fixtures/surface/adapter-config.json']=dumps({
        'surface':sid,'clock':0,'hello':hello,
        'grant':{'epoch':1,'entries':[{'coglet':cog,'operations':ops,'resources':resources,'expires':10000000}],
                 'correlation_id':cid},
        'speech_config':cfg,'initial_expression':{'expression':'neutral','intensity':0,'remaining_ms':0},
        'case_audio_timelines':'spec/fixtures/speech/timelines.json'})
    read={'type':'read','body':values['surface']['read']}
    case('surface','read-config',[{'send':read},{'expect':{'type':'read_reply','body':{
         'correlation_id':cid,'ok':True,'value':cfg,'snapshot':1}},'within_ms':200}])
    a=action('expression.set_expression',values['expression']['set-expression'])
    case('surface','action-accept-outcome',[{'send':a},accept(a),outcome(a,'succeeded')])
    stale=copy.deepcopy(a); stale['body']['grant_epoch']=0
    # Epoch zero is a schema error, so use an old but structurally valid epoch
    # after fixture regrant to epoch 2 (the test adapter's case startup mode).
    stale['body']['grant_epoch']=1
    # Separate case uses epoch 2 unknown future grant; same denial rule.
    stale['body']['grant_epoch']=2
    case('surface','grant-epoch-fence',[{'send':stale},accept(stale,False,'permission_denied')])
    foreign=copy.deepcopy(a); foreign['body']['action']['coglet']='coglet:'+'c'*64
    case('surface','per-coglet-denial',[{'send':foreign},accept(foreign,False,'permission_denied')])
    expired=copy.deepcopy(a); expired['body']['action']['deadline']=0
    case('surface','deadline-exclusive',[{'send':expired},accept(expired,False,'deadline_exceeded')])
    case('surface','dedup-terminal',[{'send':a},accept(a),outcome(a,'succeeded'),{'send':a},accept(a),outcome(a,'succeeded')])
    changed=copy.deepcopy(a); changed['body']['action']['args']['intensity']=501
    case('surface','conflicting-id',[{'send':a},accept(a),outcome(a,'succeeded'),{'send':changed},accept(changed,False,'conflict')])
    missing=action('speech.stop',{'stream':cid})
    case('surface','unknown-stream',[{'send':missing},accept(missing,False,'not_found')])
    # Fixture adapter triggers the local operator kill at virtual time 100 ms.
    case('surface','kill-latched',[{'match':{'type':'kill'},'within_ms':200,'at_ms':100},
         {'send':a,'at_ms':110},accept(a,False,'permission_denied')])
    for name,barge in [('barge-in-stop','stopped'),('over-talk-duck','ducked')]:
        a=action('speech.speak_chunk',{'stream':'2'*32,'chunk':0,'text':'Scripted output','final':True})
        steps=[{'send':a,'at_ms':0},accept(a),
               {'match':{'type':'event','body.kind':'speech.position','body.payload.state':'playing'},'within_ms':200},
               {'match':{'type':'event','body.kind':'speech.start','body.payload.utterance':cid},'within_ms':200,'at_ms':100},
               {'match':{'type':'event','body.kind':'speech.barge_in','body.payload.reflex':barge},'within_ms':200},
               {'match':{'type':'event','body.kind':'speech.position','body.payload.state':barge},'within_ms':200}]
        if barge=='stopped': steps.append(outcome(a,'cancelled'))
        steps.extend([{'match':{'type':'event','body.kind':'speech.partial'},'within_ms':200},
                      {'match':{'type':'event','body.kind':'speech.end'},'within_ms':200}])
        if barge=='ducked':
            steps.extend([{'match':{'type':'event','body.kind':'speech.position','body.payload.state':'finished'},'within_ms':200},outcome(a,'succeeded')])
        case('speech',name,steps)
    case('speech','late-partial-and-stale-revision',[
        {'match':{'type':'event','body.kind':'speech.start'},'within_ms':200},
        {'match':{'type':'event','body.kind':'speech.partial','body.payload.revision':2},'within_ms':200},
        {'match':{'type':'event','body.kind':'speech.end','body.payload.revision':3},'within_ms':200},
        {'silence_ms':150,'at_ms':200}])
    for name,offer,match in [
        ('runtime-version-selection',['0.1','0.2'],{'type':'hello_reply','body.selection.version':'0.1','body.correlation_id':cid}),
        ('runtime-no-common-version',['0.2'],{'type':'error','body.error.code':'unsupported_version','body.correlation_id':cid})]:
        greeting=copy.deepcopy(hello); greeting['body']['offer']['versions']=offer
        out[f'conformance/surface/{name}.json']=dumps({'name':name,'peer_role':'surface',
            'fixtures':['runtime-binding'],'steps':[{'send':greeting},{'match':match,'within_ms':200}]})
    return out


def main():
    check='--check' in sys.argv
    for path,content in outputs().items():
        target=ROOT/path
        if check:
            if not target.exists() or target.read_text()!=content: raise SystemExit('stale '+path)
        else:
            target.parent.mkdir(parents=True,exist_ok=True); target.write_text(content)
    print('framing and conformance case generation: OK')


if __name__=='__main__': main()
