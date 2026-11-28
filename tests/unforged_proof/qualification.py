#!/usr/bin/env python3
"""Opt-in live stage2 composition. Partial results NEVER certify stage2 PASS.

No historical-directory input, arbitrary command hooks, skip, or image/audit waiver.
Run in the locked sim Python environment under the existing credential broker.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import sys

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[1]
sys.path.insert(0,str(ROOT/'crates/coglet-gate/python'))
from guarded_transport import strict_json, digest
from prefix_inputs import step_payload
from recorded_closure import require_model_reply_closure
from verify_loopback_probe import verify

class Unavailable(ValueError):pass

NAMES=('hosted_replay_matches_recorded_step_trees','hosted_fork_matches_recorded_step_trees',
       'hosted_factory_gate_requires_fresh_execution','hosted_changed_image_invalidates_acceptance',
       'hosted_missing_receipt_fails_closed','hosted_private_export_requires_grant','hosted_replay_has_no_live_effects')

def fresh_output(path):
    try:path.mkdir(parents=True,exist_ok=False,mode=0o700)
    except FileExistsError:raise Unavailable('fresh exclusive output required; no historical results') from None

def redact(raw):
    for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
        if os.environ.get(name):raw=raw.replace(os.environ[name].encode(),b'[REDACTED]')
    return re.sub(rb'caos://[^\s]+',b'caos://[REDACTED]',raw)

def command(argv,log,*,local=False,timeout=240):
    """Fixed harness argv only; local oracle children never inherit CAOS secrets."""
    env=dict(os.environ)
    if local:
        for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):env.pop(name,None)
    log.parent.mkdir(parents=True,exist_ok=True)
    argv=list(map(str,argv))
    timed_out=False
    try:r=subprocess.run(argv,capture_output=True,env=env,timeout=timeout,cwd=ROOT)
    except subprocess.TimeoutExpired as e:
        timed_out=True;r=subprocess.CompletedProcess(argv,124,e.stdout or b'',e.stderr or b'')
    r.stdout=redact(r.stdout);r.stderr=redact(r.stderr)
    log.with_suffix('.stdout').write_bytes(r.stdout)
    log.with_suffix('.stderr').write_bytes(r.stderr)
    log.with_suffix('.json').write_bytes(redact(json.dumps(dict(argv=argv,returncode=r.returncode,
        timed_out=timed_out,local=local),indent=2).encode()))
    if timed_out:raise Unavailable('command timed out; partial evidence retained')
    return r

def require_success(result):
    if result.returncode!=0:raise Unavailable('command failed; exact diagnostics retained')
    return result.stdout

def require_refusal(result,reason):
    if result.returncode!=1 or reason.encode() not in result.stderr:
        raise Unavailable('expected refusal absent; unrelated failure is not a negative PASS')

def require_effects(trace,key):
    if not trace.get('records') or not any(r.get(key) for r in trace['records']):
        raise Unavailable('nonzero genuine recorded '+key+' required')

class Qualification:
    def __init__(self,config,output):
        required={'source','endpoint','image',*(k for n in ('driver','bridge','daemon','proof') for k in (n,n+'_sha256'))}
        if set(config)!=required:raise Unavailable('complete closed operator configuration required')
        if not isinstance(config['source'],str) or not re.fullmatch('[0-9a-f]{40}',config['source']):raise Unavailable('exact source pin required')
        if not isinstance(config['endpoint'],str) or not re.fullmatch('[0-9a-f]{64}',config['endpoint']):raise Unavailable('exact endpoint pin required')
        if not isinstance(config['image'],str) or not re.fullmatch('native@sha256:[0-9a-f]{64}',config['image']):raise Unavailable('exact native image pin required')
        self.c=dict(config)
        for name in ('driver','bridge','daemon','proof'):
            pin=config[name+'_sha256']
            if not isinstance(pin,str) or not re.fullmatch('[0-9a-f]{64}',pin):raise Unavailable('binary pin required')
            try:
                path=Path(config[name]).resolve(strict=True)
                if digest(path.read_bytes())!=pin:raise Unavailable('independent '+name+' binary pin differs')
            except (OSError,TypeError):raise Unavailable('operator binary unavailable') from None
            if not os.access(path,os.X_OK):raise Unavailable('operator binary not executable')
            self.c[name]=str(path)
        clean={k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
        head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,env=clean).decode().strip()
        dirty=subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=ROOT,env=clean)
        if head!=config['source'] or dirty:raise Unavailable('wrong or dirty containing source')
        if not os.environ.get('CAOS_IROH_TICKET'):raise Unavailable('hosted credential configuration unavailable')
        self.out=Path(output).resolve().absolute() if not Path(output).is_symlink() else Path(output).absolute()
        fresh_output(self.out)
        (self.out/'config.json').write_text(json.dumps(self.c,indent=2)+'\n')
        self.cache={}

    def cmd(self,label,argv,*,local=False,timeout=240):
        return command(argv,self.out/'commands'/label,local=local,timeout=timeout)

    def python(self,label,script,*args,local=False,timeout=240):
        return self.cmd(label,[sys.executable,HERE/script,*args],local=local,timeout=timeout)

    def native(self,label,*args):
        return require_success(self.cmd(label,[self.c['driver'],*args],local=True))

    def model(self):
        if 'model' not in self.cache:
            raw=self.native('record-model','record-model',ROOT)
            trace=strict_json(raw);require_effects(trace,'llm_requests');require_model_reply_closure(trace)
            path=self.out/'model.json';path.write_bytes(raw)
            self.cache['model']=self.execute_trace('model',path,'model')
        return self.cache['model']

    def oracles(self):
        if 'oracles' not in self.cache:
            require_success(self.python('record-oracles','record_oracles.py','--driver',self.c['driver'],
                '--output',self.out/'oracles',local=True))
            replay=strict_json((self.out/'oracles/replay-trace.json').read_bytes())
            fork=strict_json((self.out/'oracles/fork-trace.json').read_bytes())
            require_effects(replay,'actions');require_effects(fork,'actions')
            if replay['final_hash']==fork['final_hash']:raise Unavailable('fork oracle did not diverge')
            self.cache['oracles']=True

    def prepared_args(self,path,snapshot,index,oracle,job):
        trace=strict_json(path.read_bytes())
        return ['--synthetic-transport-only','--trace',path,'--snapshot',snapshot,
            '--driver',self.c['driver'],'--bridge',self.c['bridge'],'--output',job,
            '--expected-endpoint',self.c['endpoint'],'--expected-image',self.c['image'],
            '--expected-bridge-sha256',self.c['bridge_sha256'],'--coglet',trace['coglet'],
            '--step',str(index),'--oracle',oracle]

    def execute_trace(self,label,path,oracle):
        trace=strict_json(path.read_bytes());require_model_reply_closure(trace)
        if not trace.get('records'):raise Unavailable('empty oracle')
        checked=[]
        for index,record in enumerate(trace['records']):
            prefix=self.out/f'{label}-{index}-prefix.json'
            prefix.write_text(json.dumps(step_payload(trace,index)['prefix']))
            snapshot=self.out/f'{label}-{index}-snapshot.json'
            snapshot.write_bytes(self.native(f'{label}-{index}-snapshot','snapshot',prefix))
            job=self.out/f'{label}-{index}'
            require_success(self.python(f'{label}-{index}-execute','prepared_probe.py',
                *self.prepared_args(path,snapshot,index,oracle,job)))
            # Separate implementation reads raw authenticated-client observations,
            # completed status/sidecar and EVERY returned tree/blob, no projection.
            checked.append(verify(job,path,index,image=self.c['image'],source=self.c['source'],
                driver_hash=self.c['driver_sha256'],bridge_hash=self.c['bridge_sha256']))
        if len({r['request'] for r in checked})!=len(checked):raise Unavailable('duplicate execution identity')
        result=dict(trace=str(path.relative_to(self.out)),steps=checked)
        (self.out/f'{label}-verification.json').write_text(json.dumps(result,indent=2)+'\n')
        return result

    def hosted_replay_matches_recorded_step_trees(self):
        self.model();self.oracles()
        if 'replay' not in self.cache:
            self.cache['replay']=self.execute_trace('replay',self.out/'oracles/replay-trace.json','gridbot')
        return self.cache['replay']

    def hosted_fork_matches_recorded_step_trees(self):
        self.oracles()
        if 'fork' not in self.cache:
            self.cache['fork']=self.execute_trace('fork',self.out/'oracles/fork-trace.json','gridbot')
        return self.cache['fork']

    def hosted_factory_gate_requires_fresh_execution(self):
        summaries=[]
        for label,limit,expected in [('factory-pass',16,True),('factory-reject',1,False)]:
            job=self.out/label
            argv=['--synthetic-transport-only','--output',job,'--max-steps',str(limit),
                '--expected-endpoint',self.c['endpoint'],'--expected-image',self.c['image']]
            for n in ('driver','bridge','daemon','proof'):argv+=['--'+n,self.c[n]]
            for n in ('driver','bridge'):argv+=['--expected-'+n+'-sha256',self.c[n+'_sha256']]
            require_success(self.python(label,'factory_probe.py',*argv,timeout=1200))
            require_success(self.python(label+'-verify','verify_factory_probe.py','--batch',job,
                '--require-artifact','--expected-image',self.c['image'],local=True))
            summary=strict_json((job/'result.json').read_bytes())
            if summary['behavioral_passed'] is not expected:raise Unavailable('factory behavioral verdict differs')
            command_pin=strict_json((job/'command.json').read_bytes())
            for n in ('daemon','proof'):
                if command_pin[n+'_sha256']!=self.c[n+'_sha256']:raise Unavailable('native factory tool pin differs')
            summaries.append(summary)
        return summaries

    def receipt_negatives(self):
        if 'receipt-negatives' in self.cache:return self.cache['receipt-negatives']
        self.model()
        job=self.out/'model-0';path=self.out/'model.json'
        original=strict_json((job/'result.json').read_bytes())
        raw=(job/'bridge.jsonl').read_bytes();rows=[strict_json(line) for line in raw.splitlines()]
        executed=[r for r in rows if r.get('phase')=='executed']
        if len(executed)!=1:raise Unavailable('genuine receipt absent')
        # Mutate isolated copies only; original authenticated observations stay exact.
        mutations={'missing':[r for r in rows if r.get('phase')!='executed'],
                   'ambiguous':rows+executed,'wrong-image':copy.deepcopy(rows)}
        for row in mutations['wrong-image']:
            if row.get('phase')=='executed':row['worker_image']='native@sha256:'+'0'*64
        names=[]
        for name,altered in mutations.items():
            clone=self.out/('LOCAL-'+name);clone.mkdir()
            for file in ('result.json','command.json','scope.json'):(clone/file).write_bytes((job/file).read_bytes())
            (clone/'bridge.jsonl').write_bytes(b'\n'.join(json.dumps(r).encode() for r in altered)+b'\n')
            try:verify(clone,path,0,image=self.c['image'],source=self.c['source'],
                       driver_hash=self.c['driver_sha256'],bridge_hash=self.c['bridge_sha256'])
            except (ValueError,KeyError):names.append(name)
            else:raise Unavailable('LOCAL negative accepted: '+name)
        self.cache['receipt-negatives']=dict(LOCAL_refusals=names,original_request=original['request'],actual_worker_upgrade=False)
        return self.cache['receipt-negatives']

    def hosted_changed_image_invalidates_acceptance(self):
        self.receipt_negatives()
        raise Unavailable('actual retained-client worker image transition unavailable; changed-pin refusal is NOT upgrade qualification')

    def hosted_missing_receipt_fails_closed(self):
        return self.receipt_negatives()

    def hosted_private_export_requires_grant(self):
        self.model()
        path=self.out/'model.json';snapshot=self.out/'model-0-snapshot.json';job=self.out/'denied-export'
        result=self.python('denied-export','prepared_probe.py',*self.prepared_args(path,snapshot,0,'model',job),'--deny-export',local=True)
        denied=strict_json(require_success(result))
        if denied!=dict(denied=True,native_started=False,bridge_started=False,private_staging_writes=0) or job.exists():
            raise Unavailable('denial did not prevent preparation/transport')
        trace=strict_json(path.read_bytes())
        # A genuine prefix containing the request but not its recorded reply.
        trace['records']=trace['records'][:1];trace['final_hash']=trace['records'][0]['output_hash']
        trace.pop('trailing_boundary',None)
        missing=self.out/'missing-reply.json';missing.write_text(json.dumps(trace))
        target=self.out/'missing-reply-output'
        result=self.python('missing-reply','prepared_probe.py',*self.prepared_args(missing,snapshot,0,'model',target),local=True)
        require_refusal(result,'ValueError: missing recorded model reply')
        if target.exists():raise Unavailable('missing reply created output')
        return dict(denied=denied,missing_reply_refused=True)

    def hosted_replay_has_no_live_effects(self):
        from no_effects_closure import inspect
        from no_effects_policy import check
        # These are committed reviewed-source bytes, never an operator waiver.
        # Check before any execution, and retain raw selected Cargo graph.
        pins=check();graph=inspect()
        (self.out/'no-effects-closure.json').write_text(json.dumps(graph,indent=2)+'\n')
        self.hosted_replay_matches_recorded_step_trees()
        model=strict_json((self.out/'model.json').read_bytes())
        replay=strict_json((self.out/'oracles/replay-trace.json').read_bytes())
        require_effects(model,'llm_requests');require_effects(replay,'actions')
        note=dict(source=self.c['source'],model_steps=len(model['records']),adapter_steps=len(replay['records']),
                  audited_source_files=len(pins),source_pins=pins,
                  observed='each step required strict namespace and synthetic loopback controls',
                  claim='bare Runtime.step records guest model/actions; only recorded boundary outcomes enter replay; no reachable live dispatcher in reviewed source',
                  limitation='not universal syscall accounting; loopback controls cover synthetic endpoints only')
        (self.out/'no-effects-source-audit.json').write_text(json.dumps(note,indent=2)+'\n')
        return note

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    p.add_argument('--case',action='append',choices=NAMES)
    a=p.parse_args();q=Qualification(strict_json(a.config.read_bytes()),a.output)
    results={}
    for name in dict.fromkeys(a.case or NAMES):
        try:results[name]=dict(status='PASS',evidence=getattr(q,name)())
        except Exception as e:
            # Preserve bounded error type/reason, never convert a crash into PASS.
            results[name]=dict(status='FAIL',error=type(e).__name__,reason=redact(str(e).encode()).decode())
        (q.out/'qualification.json').write_text(json.dumps(dict(source=q.c['source'],cases=results,
            stage2_pass=False,remaining=['actual retained-client worker image transition','exact-head no-effects audit',
             'separate native warm/resolver scope','required CI/native queue/main/containing-source proof']),indent=2)+'\n')
    print(json.dumps(results,indent=2))
    return 1 if any(r['status']!='PASS' for r in results.values()) else 0

if __name__=='__main__':raise SystemExit(main())
