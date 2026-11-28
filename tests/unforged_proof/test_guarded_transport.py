"""Testbed recipe authority, not hosted qualification."""
import copy
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, Refused, TrustKey
from guarded_transport import RecipeAuthority, recipe_for, scope_for, ApprovalSession


class GuardedTransportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.revoked = set()
        self.authority = RecipeAuthority(ArtifactStore(self.tmp.name), TrustKey('export', b'r'*32), self.revoked)
        self.recipe = recipe_for({'data/value': b'private'}, ['data', 'empty'], driver=b'\x7fELFtest', worker=b'worker', salt='a'*64, image='native@sha256:'+'b'*64)
        self.scope = scope_for(self.recipe, sender='c'*64, recipient='d'*64, coglet='coglet:'+'e'*64)
        self.grant = self.authority.issue(self.scope)

    def session(self, grant=None):
        return ApprovalSession(self.authority, grant or self.grant, self.scope)

    def initial(self):
        return dict(phase='authorize',sequence=1,operation='initial',endpoint='d'*64,recipe=self.recipe)

    def test_complete_recipe_retains_empty_dirs_and_all_wrappers(self):
        nodes = {n['path']:n for n in self.recipe['nodes']}
        self.assertEqual(nodes['args/state/empty'], dict(path='args/state/empty',mode='40000'))
        self.assertEqual(nodes['args/state/data/value']['sha256'], hashlib.sha256(b'private').hexdigest())
        self.assertEqual(nodes['args/required-pool']['sha256'], hashlib.sha256(b'unforged').hexdigest())
        self.assertEqual(set(nodes), {'args','args/driver','args/worker1','args/salt','args/required-native-worker-image','args/required-pool','args/state','args/state/data','args/state/data/value','args/state/empty'})

    def test_denied_and_wrong_target_grants_refuse_before_approval(self):
        for grant in (None, 'f'*64):
            with self.assertRaises(Refused):
                ApprovalSession(self.authority, grant, self.scope)
        for key in ('sender','recipient','coglet'):
            scope = copy.deepcopy(self.scope)
            scope[key] = ('coglet:' if key == 'coglet' else '')+'f'*64
            with self.assertRaises(Refused):
                ApprovalSession(self.authority, self.grant, scope)

    def test_changed_retained_bytes_or_derived_wrapper_refuse(self):
        for key,value in [('image','native@sha256:'+'f'*64),('salt','f'*64),('engine','f'*64),('base',dict(type='docker',value='different'))]:
            event = self.initial()
            event['recipe'] = copy.deepcopy(self.recipe)
            event['recipe'][key] = value
            with self.subTest(key=key),self.assertRaises(Refused):
                self.session().approve(event)
        event = self.initial()
        event['recipe'] = copy.deepcopy(self.recipe)
        event['recipe']['nodes'][-1]['sha256'] = 'f'*64
        with self.assertRaises(Refused): self.session().approve(event)

    def test_revocation_stops_next_crossing(self):
        session = self.session()
        self.assertEqual(session.approve(self.initial()), dict(approve=1))
        event = dict(phase='authorize',sequence=2,operation='serialize',label='blob:args/driver')
        self.assertEqual(session.approve(event),dict(approve=2))
        self.revoked.add(self.grant)
        with self.assertRaises(Refused):
            session.approve(dict(phase='authorize',sequence=3,operation='request',method='HEAD',path='/object/'+'a'*40,body_sha256=None))

    def test_sequence_unknown_operation_and_endpoint_fail_closed(self):
        bad = [dict(self.initial(),sequence=True),dict(self.initial(),sequence=2),dict(self.initial(),endpoint='f'*64),dict(self.initial(),extra=1)]
        for event in bad:
            with self.assertRaises(Refused): self.session().approve(event)
        session = self.session()
        session.approve(self.initial())
        with self.assertRaises(Refused): session.approve(self.initial())
        with self.assertRaises(Refused): session.approve(dict(phase='authorize',sequence=2,operation='arbitrary'))

    def test_malformed_row_always_reaps_child(self):
        import subprocess
        from unittest.mock import patch
        from guarded_transport import run_guarded
        script = Path(self.tmp.name)/'malformed-bridge'
        script.write_text('#!/usr/bin/env python3\nimport time\nprint("[1]",flush=True)\ntime.sleep(60)\n')
        script.chmod(0o700)
        children = []
        popen = subprocess.Popen
        def start(*a,**kw):
            child = popen(*a,**kw)
            children.append(child)
            return child
        try:
            with patch('guarded_transport.subprocess.Popen',side_effect=start):
                result = run_guarded([str(script)],self.strong(),bridge_sha256=hashlib.sha256(script.read_bytes()).hexdigest(),timeout=2)
            self.assertIsNotNone(result['refusal'])
            self.assertIsNotNone(children[0].poll())
        finally:
            for child in children:
                if child.poll() is None: child.kill()
                child.wait()

    def test_invalid_method_path_pair_refuses(self):
        for method,path in [('POST','/version'),('GET','/submit'),('POST','/object/'+'a'*40)]:
            session = self.session()
            session.approve(self.initial())
            with self.subTest(method=method,path=path),self.assertRaises(Refused):
                session.approve(dict(phase='authorize',sequence=2,operation='request',method=method,path=path,body_sha256=None))

    def test_recipe_rejects_unsafe_paths_aliases_and_missing_parent(self):
        for path in ('../x','.git/x','a//b','/absolute','data','bad\\name'):
            with self.subTest(path=path),self.assertRaises(Refused):
                recipe_for({path:b'x'}, ['data'], driver=b'\x7fELFx',worker=b'w',salt='a'*64,image='native@sha256:'+'b'*64)
        scope = copy.deepcopy(self.scope)
        scope['recipe']['nodes'].append(copy.deepcopy(scope['recipe']['nodes'][0]))
        with self.assertRaises(Refused): self.authority.issue(scope)

    # ---- strong verifier: honest bridge transcript model (built with wire_trees, not the session)
    def retained(self):
        return {'args/driver':b'\x7fELFtest','args/worker1':b'worker','args/salt':('a'*64).encode(),
                'args/required-native-worker-image':('native@sha256:'+'b'*64).encode(),
                'args/required-pool':b'unforged','args/state/data/value':b'private'}

    def strong(self,**kw):
        return ApprovalSession(self.authority,self.grant,self.scope,retained_blobs=kw.get('retained',self.retained()))

    def transcript(self,hit=()):
        """Honest bridge: list of ('auth',fields) and ('row',row) in Rust order."""
        from wire_trees import object_id
        import json as _json
        nodes = {n['path']:n for n in self.recipe['nodes']}
        raw = self.retained()
        items,ids = [],{}
        def wire(kind,payload): return kind.encode()+b' '+str(len(payload)).encode()+b'\0'+payload
        def put(label,kind,payload):
            oid = object_id(kind,payload)
            items.append(('auth',dict(operation='serialize',label=label)))
            items.append(('auth',dict(operation='request',method='HEAD',path='/object/'+oid,body_sha256=None)))
            if label not in hit:
                items.append(('auth',dict(operation='request',method='POST',path='/object/',body_sha256=hashlib.sha256(wire(kind,payload)).hexdigest())))
            return oid
        def build(path):
            if nodes[path]['mode'] == '40000':
                kids = [p for p in nodes if p != 'args' and p.rpartition('/')[0] == path]
                kids.sort(key=lambda p:p.rpartition('/')[2].encode())
                # children are uploaded in name order; the tree body is in git order
                for k in kids: build(k)
                kids.sort(key=lambda p:(p.rpartition('/')[2]+('/' if nodes[p]['mode']=='40000' else '')).encode())
                body = b''.join(nodes[k]['mode'].encode()+b' '+k.rpartition('/')[2].encode()+b'\0'+bytes.fromhex(ids[k]) for k in kids)
                ids[path] = put('tree:'+path,'tree',body)
            else: ids[path] = put('blob:'+path,'blob',raw[path])
        build('args')
        bundle = put('tree:bundle','tree',b'40000 args\0'+bytes.fromhex(ids['args']))
        base = self.recipe['base']
        body = _json.dumps(dict(base=base,bundle=bundle,salt='a'*64),separators=(',',':')).encode()
        items.append(('auth',dict(operation='request',method='POST',path='/submit',body_sha256=hashlib.sha256(body).hexdigest())))
        req = 'c1'*20
        items.append(('row',dict(phase='submitted',request=req,salt='a'*64,args=ids['args'],state=ids['args/state'],driver=ids['args/driver'],worker=ids['args/worker1'],expected_image=self.recipe['image'],bundle=bundle,pool='unforged')))
        blob = b'out'
        boid = object_id('blob',blob)
        tree = b'100644 a\0'+bytes.fromhex(boid)
        root = object_id('tree',tree)
        side = b'{"x":1}'
        soid = object_id('blob',side)
        items.append(('auth',dict(operation='request',method='GET',path='/run?req=%s&nocache=1'%req,body_sha256=None)))
        items.append(('row',dict(phase='executed',request=req,result_oid=root,cache_hit=False,worker_image=self.recipe['image'],server_confirmed_nocache=True)))
        status = _json.dumps(dict(arg_tree=req,out_trace=[soid]))
        for path,body_ in (('/version','1'),('/status/'+req,'{}'),('/status/%s?all=1'%req,status)):
            items.append(('auth',dict(operation='request',method='GET',path=path,body_sha256=None)))
            items.append(('row',dict(phase='readback',path=path,http_status=200,body=body_)))
        items.append(('auth',dict(operation='request',method='GET',path='/object/'+soid,body_sha256=None)))
        items.append(('row',dict(phase='sidecar-readback',request=req,oid=soid,raw_payload=side.decode())))
        items.append(('auth',dict(operation='request',method='GET',path='/object/'+root,body_sha256=None)))
        items.append(('row',dict(phase='abi-object',oid=root,kind='tree',hex=tree.hex())))
        items.append(('auth',dict(operation='request',method='GET',path='/object/'+boid,body_sha256=None)))
        items.append(('row',dict(phase='abi-object',oid=boid,kind='blob',hex=blob.hex())))
        items.append(('row',dict(phase='abi-file',path='a',mode='100644',hex=blob.hex())))
        return items

    def play(self,session,items):
        seq = 0
        for kind,value in items:
            if kind == 'auth':
                seq += 1
                if seq == 1: value = dict(operation='initial',endpoint='d'*64,recipe=self.recipe)
                session.approve(dict(phase='authorize',sequence=seq,**value))
            else: session.observe(value)
        session.finish()

    def full(self,hit=()):
        return [('auth',dict(operation='initial'))]+self.transcript(hit)

    def test_honest_transcript_verifies_including_hit_and_miss(self):
        for hit in ((),('blob:args/driver','tree:args/state')):
            session = self.strong()
            self.play(session,self.full(hit))
            self.assertEqual(session.request,'c1'*20)
            self.assertEqual(len(session.sidecar_oid),40)

    def test_retained_bytes_must_match_recipe_before_first_ack(self):
        for mutate in (lambda r:r.update({'args/driver':b'\x7fELFevil'}),lambda r:r.pop('args/state/data/value'),
                       lambda r:r.update({'args/extra':b'x'}),lambda r:r.update({'args/salt':b'z'*64}),lambda r:r.update({'args/state/data/value':'private'})):
            retained = self.retained(); mutate(retained)
            with self.assertRaises(Refused): self.strong(retained=retained).approve(self.initial())

    def mutated(self,index,fn):
        items = self.full()
        kind,value = items[index]
        items[index] = (kind,fn(dict(value)))
        return items

    def test_ordering_repeat_and_body_mismatch_refuse(self):
        items = self.full()
        def idx(pred): return next(i for i,(k,v) in enumerate(items) if pred(k,v))
        post = idx(lambda k,v:k=='auth' and v.get('method') == 'POST' and v['path'] == '/object/')
        head = idx(lambda k,v:k=='auth' and v.get('method') == 'HEAD')
        sub = idx(lambda k,v:k=='auth' and v.get('path') == '/submit')
        cases = {
            'wrong post body': self.mutated(post,lambda v:dict(v,body_sha256='f'*64)),
            'wrong head oid': self.mutated(head,lambda v:dict(v,path='/object/'+'f'*40)),
            'GET instead of HEAD': self.mutated(head,lambda v:dict(v,method='GET')),
            'wrong submit body': self.mutated(sub,lambda v:dict(v,body_sha256='f'*64)),
            'wrong label': self.mutated(1,lambda v:dict(v,label='blob:args/salt')),
            'repeat serialize': items[:post]+[items[1]]+items[post:],
            'repeat post': items[:post+1]+[items[post]]+items[post+1:],
            'submit before bundle': items[:sub-3]+[items[sub]]+items[sub-3:sub],
            'second submit': items[:sub+2]+[items[sub]]+items[sub+2:],
            'serialize after submit': items[:sub+2]+[('auth',dict(operation='serialize',label='tree:bundle'))]+items[sub+2:],
        }
        for name,case in cases.items():
            with self.subTest(name),self.assertRaises(Refused): self.play(self.strong(),case)

    def test_rows_and_readbacks_are_bound_to_requests(self):
        items = self.full()
        def at(phase,occurrence=0):
            return [i for i,(k,v) in enumerate(items) if k == 'row' and v['phase'] == phase][occurrence]
        def reqat(path_prefix):
            return next(i for i,(k,v) in enumerate(items) if k == 'auth' and v.get('path','').startswith(path_prefix))
        sub,ex,side = at('submitted'),at('executed'),at('sidecar-readback')
        cases = {
            'submitted state differs': self.mutated(sub,lambda v:dict(v,state='f'*40)),
            'submitted driver differs': self.mutated(sub,lambda v:dict(v,driver='f'*40)),
            'submitted bundle differs': self.mutated(sub,lambda v:dict(v,bundle='f'*40)),
            'submitted salt differs': self.mutated(sub,lambda v:dict(v,salt='f'*64)),
            'submitted malformed request': self.mutated(sub,lambda v:dict(v,request='C'*40)),
            'run another request': self.mutated(reqat('/run'),lambda v:dict(v,path='/run?req=%s&nocache=1'%('d'*40))),
            'executed cache hit': self.mutated(ex,lambda v:dict(v,cache_hit=True)),
            'executed wrong image': self.mutated(ex,lambda v:dict(v,worker_image='native@sha256:'+'f'*64)),
            'executed wrong request': self.mutated(ex,lambda v:dict(v,request='d'*40)),
            'executed no nocache confirm': self.mutated(ex,lambda v:dict(v,server_confirmed_nocache=False)),
            'readback of other path': self.mutated(at('readback'),lambda v:dict(v,path='/status/'+'d'*40)),
            'sidecar other oid': self.mutated(side,lambda v:dict(v,oid='e'*40)),
            'sidecar payload differs': self.mutated(side,lambda v:dict(v,raw_payload='{"x":2}')),
            'sidecar other request': self.mutated(side,lambda v:dict(v,request='d'*40)),
            'object bytes differ': self.mutated(at('abi-object'),lambda v:dict(v,hex='00')),
            'abi-file differs': self.mutated(at('abi-file'),lambda v:dict(v,hex='00')),
            'extra unrequested object row': items[:at('abi-object')]+[items[at('abi-object')]]*2+items[at('abi-object')+1:],
            'unknown phase': items[:sub]+[('row',dict(phase='mystery'))]+items[sub:],
            'unrequested object GET': items[:-3]+[('auth',dict(operation='request',method='GET',path='/object/'+'e'*40,body_sha256=None))]+items[-3:],
            'sidecar never fetched': items[:side-1]+items[side+1:],
        }
        for name,case in cases.items():
            with self.subTest(name),self.assertRaises(Refused): self.play(self.strong(),case)

    def test_incomplete_transcript_does_not_finish(self):
        with self.assertRaises(Refused): self.play(self.strong(),self.full()[:-3])

    def test_revocation_before_expected_object_construction(self):
        session = self.strong()
        session.approve(self.initial())
        self.revoked.add(self.grant)
        with self.assertRaises(Refused):
            session.approve(dict(phase='authorize',sequence=2,operation='serialize',label='blob:args/driver'))
        self.assertEqual(session.objects,{})

    def bridge_script(self,items,name='bridge'):
        import json as _json
        script = Path(self.tmp.name)/name
        recipe = Path(self.tmp.name)/'recipe.json'
        recipe.write_text(_json.dumps(self.recipe))
        script.write_text('#!/usr/bin/env python3\nimport json,sys\nseq=0\nfor kind,v in json.loads(%r):\n    if kind=="auth":\n        seq+=1\n        if seq==1: v=dict(operation="initial",endpoint="d"*64,recipe=json.load(open(%r)))\n        print(json.dumps(dict(phase="authorize",sequence=seq,**v)),flush=True)\n        if json.loads(sys.stdin.readline())!=dict(approve=seq): sys.exit(3)\n    else: print(json.dumps(v),flush=True)\n' % (_json.dumps(items),str(recipe)))
        script.chmod(0o700)
        return script

    def test_real_process_handshake_and_mandatory_mapping(self):
        from guarded_transport import run_guarded
        script = self.bridge_script(self.full())
        pin = hashlib.sha256(script.read_bytes()).hexdigest()
        result = run_guarded([str(script)],self.strong(),bridge_sha256=pin,timeout=20)
        self.assertEqual((result['refusal'],result['returncode']),(None,0))
        self.assertEqual(result['approvals'],sum(1 for k,_ in self.full() if k == 'auth'))
        with self.assertRaises(Refused):   # weak session cannot start any real process
            run_guarded([str(script)],self.session(),bridge_sha256=pin,timeout=5)
        bad = self.full()
        bad[2] = ('auth',dict(operation='serialize',label='blob:args/salt'))
        script = self.bridge_script(bad,'bridge2')
        result = run_guarded([str(script)],self.strong(),bridge_sha256=hashlib.sha256(script.read_bytes()).hexdigest(),timeout=20)
        self.assertIsNotNone(result['refusal'])

    def test_baseexception_still_kills_and_reaps_child(self):
        import subprocess
        from unittest.mock import patch
        from guarded_transport import run_guarded
        script = Path(self.tmp.name)/'sleeper'
        script.write_text('#!/usr/bin/env python3\nimport time\nprint("{\\"phase\\":\\"x\\"}",flush=True)\ntime.sleep(60)\n')
        script.chmod(0o700)
        children,popen = [],subprocess.Popen
        def start(*a,**kw):
            children.append(popen(*a,**kw)); return children[-1]
        def boom(event): raise KeyboardInterrupt()
        try:
            with patch('guarded_transport.subprocess.Popen',side_effect=start),self.assertRaises(KeyboardInterrupt):
                run_guarded([str(script)],self.strong(),bridge_sha256=hashlib.sha256(script.read_bytes()).hexdigest(),timeout=5,on_event=boom)
            self.assertIsNotNone(children[0].poll())
        finally:
            for child in children:
                if child.poll() is None: child.kill()
                child.wait()


if __name__ == '__main__': unittest.main()
