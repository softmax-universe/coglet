"""Operator-owned testbed recipe grants for the existing STANDARD transport.

Recipe hashes are derivation metadata, NOT unmaterialized Git tree OIDs. Keys and
revocation state stay in the operator process. A pinned trusted bridge retains
bytes, validates its recipe against this independently constructed one, then waits
before every private wire serialization and every network request. Revocation
stops a subsequent crossing; it cannot retract prior disclosures or execution.
"""
import hashlib
import json
import os
from pathlib import PurePosixPath
import re
import selectors
import subprocess
import time
from coglet_gate import Refused, canonical

NATIVE_BASE = {'type':'docker','value':'unforged@sha256:756e666f726765643a63616f732d72756e6e65723a76312e3100000000000000'}
HEX = re.compile('[0-9a-f]{64}')

def digest(raw):
    return hashlib.sha256(raw).hexdigest()

def _hex(value):
    return isinstance(value,str) and HEX.fullmatch(value) is not None

def _path(path):
    if (not isinstance(path,str) or not path or '\\' in path or '\0' in path
            or str(PurePosixPath(path)) != path or path.startswith('/')
            or any(p in ('.','..','.git','') for p in path.split('/'))):
        raise Refused('invalid recipe path')
    return path

def recipe_for(files, directories, *, driver, worker, salt, image):
    """Hash retained raw leaves, no private tree/wire serialization or file writes.

    Input dict byte values must already be held/validated by the trusted caller.
    This builder does not fetch private payloads or trust candidate descriptors.
    """
    entries = {}
    def directory(path):
        path = _path(path)
        if path in entries and entries[path]['mode'] != '40000':
            raise Refused('file/directory recipe collision')
        entries[path] = dict(path=path,mode='40000')
    def blob(path,raw,mode='100644'):
        path = _path(path)
        if path in entries or type(raw) is not bytes:
            raise Refused('ambiguous recipe leaf')
        entries[path] = dict(path=path,mode=mode,sha256=digest(raw),length=len(raw))
    directory('args')
    directory('args/state')
    for path in directories:
        directory('args/state/'+_path(path))
    for path,raw in files.items():
        path = 'args/state/'+_path(path)
        # Require every directory explicitly, including otherwise empty ones.
        if str(PurePosixPath(path).parent) not in entries:
            raise Refused('recipe parent missing')
        blob(path,raw)
    for path,raw,mode in [('driver',driver,'100755'),('worker1',worker,'100755'),('salt',salt.encode(),'100644'),('required-native-worker-image',image.encode(),'100644'),('required-pool',b'unforged','100644')]:
        blob('args/'+path,raw,mode)
    recipe = dict(version=1,base=dict(NATIVE_BASE),engine=None,nodes=[entries[k] for k in sorted(entries)],salt=salt,image=image)
    _recipe(recipe)
    return recipe

def _recipe(recipe):
    if (not isinstance(recipe,dict) or set(recipe) != {'version','base','engine','nodes','salt','image'}
            or type(recipe['version']) is not int or recipe['version'] != 1
            or recipe['base'] != NATIVE_BASE or recipe['engine'] is not None
            or not _hex(recipe['salt']) or not isinstance(recipe['image'],str)
            or not recipe['image'].startswith('native@sha256:') or not _hex(recipe['image'][14:])):
        raise Refused('closed native recipe required')
    nodes = recipe['nodes']
    if not isinstance(nodes,list) or not 8 <= len(nodes) <= 100000:
        raise Refused('bounded complete recipe required')
    entries = {}
    total = 0
    for node in nodes:
        if not isinstance(node,dict): raise Refused('recipe node required')
        path = _path(node.get('path'))
        if path in entries or not (path == 'args' or path.startswith('args/')):
            raise Refused('duplicate or escaped recipe node')
        if node.get('mode') == '40000':
            if set(node) != {'path','mode'}: raise Refused('closed recipe directory required')
        elif node.get('mode') in ('100644','100755'):
            if (set(node) != {'path','mode','sha256','length'} or not _hex(node['sha256'])
                    or type(node['length']) is not int or not 0 <= node['length'] <= 256*1024*1024):
                raise Refused('invalid raw leaf identity')
            total += node['length']
        else: raise Refused('unsupported recipe mode')
        entries[path] = node
    if list(entries) != sorted(entries) or total > 321*1024*1024:
        raise Refused('recipe order/size invalid')
    for path,node in entries.items():
        if path != 'args' and entries.get(str(PurePosixPath(path).parent),{}).get('mode') != '40000':
            raise Refused('recipe parent missing')
        if path.startswith('args/state/') and node['mode'] not in ('40000','100644'):
            raise Refused('state recipe mode invalid')
    top = {'args','args/state','args/driver','args/worker1','args/salt','args/required-native-worker-image','args/required-pool'}
    if set(p for p in entries if not p.startswith('args/state/')) != top:
        raise Refused('incomplete or extra wrapper')
    for path in ('args','args/state'):
        if entries[path]['mode'] != '40000': raise Refused('wrapper directory required')
    for path in ('args/driver','args/worker1'):
        if entries[path]['mode'] != '100755': raise Refused('executable wrapper required')
    for path,raw in [('args/salt',recipe['salt'].encode()),('args/required-native-worker-image',recipe['image'].encode()),('args/required-pool',b'unforged')]:
        if entries[path] != dict(path=path,mode='100644',sha256=digest(raw),length=len(raw)):
            raise Refused('wrapper bytes differ')
    return recipe

def scope_for(recipe, *, sender, recipient, coglet):
    return _scope(dict(version=1,sender=sender,recipient=recipient,coglet=coglet,operations=['export','read'],recipe=recipe))

def _scope(scope):
    if (not isinstance(scope,dict) or set(scope) != {'version','sender','recipient','coglet','operations','recipe'}
            or type(scope['version']) is not int or scope['version'] != 1
            or not _hex(scope['sender']) or not _hex(scope['recipient'])
            or not isinstance(scope['coglet'],str) or not scope['coglet'].startswith('coglet:') or not _hex(scope['coglet'][7:])
            or scope['operations'] != ['export','read']):
        raise Refused('closed recipe export/read scope required')
    _recipe(scope['recipe'])
    # Canonicalize metadata only; private bytes are not contained here.
    return json.loads(canonical(scope))

class RecipeAuthority:
    def __init__(self,store,key,revoked):
        self.store,self.key,self.revoked = store,key,revoked
    def issue(self,scope):
        return self.store.put(_scope(scope),self.key,'hosted-private-recipe-v1')
    def verify(self,grant,scope):
        expected = _scope(scope)
        if not isinstance(grant,str) or grant in self.revoked:
            raise Refused('recipe grant missing/revoked')
        if self.store.read(grant,self.key,'hosted-private-recipe-v1') != expected:
            raise Refused('recipe grant scope mismatch')

OID = re.compile('[0-9a-f]{40}')
MAX_OUTPUT = 64*1024*1024

def _oid(value):
    return isinstance(value,str) and OID.fullmatch(value) is not None

def _wire(kind,payload):
    """Git canonical `<kind> <len>\\0payload` -> (sha1 oid, sha256 abi oid, wire bytes)."""
    wire = kind.encode()+b' '+str(len(payload)).encode()+b'\0'+payload
    return hashlib.sha1(wire).hexdigest(),hashlib.sha256(wire).hexdigest(),wire

def _json_str(value):
    return json.dumps(value,ensure_ascii=False)

def _name(value):
    return (isinstance(value,str) and value not in ('','.','..','.git')
            and not any(c in value for c in '/\\\0'))

_ROWS = {'submitted':'submitted','executed':'executed','readback':'readback','sidecar':'sidecar-readback',
         'object':'abi-object','abi-file':'abi-file'}

class ApprovalSession:
    """Independent verifier of every private crossing of a pinned trusted bridge.

    Pump API: `approve(authorize_event) -> ack` for each authorize row, `observe(row)`
    for EVERY other stdout row, `finish()` after a clean bridge exit (run_guarded does
    all three). `retained_blobs` maps each recipe blob path (`args/driver`,
    `args/state/x`, ...) to its raw bytes and is mandatory for `run_guarded`.
    Without it only message shape is checked (unit-test mode, `strong` False);
    run_guarded refuses such a session before spawning anything.

    Expected wire objects are serialized here only after the initial verify, and the
    grant is re-verified before every approval/construction. Traversal mirrors the
    bridge: depth-first, children in byte order, each object [HEAD [POST]], bundle last.
    """
    def __init__(self,authority,grant,scope,retained_blobs=None):
        self.scope = _scope(scope)
        self.authority,self.grant,self.sequence = authority,grant,0
        self.retained = None
        if retained_blobs is not None:
            if not isinstance(retained_blobs,dict): raise Refused('retained blob mapping required')
            self.retained = dict(retained_blobs)
        self.strong = self.retained is not None
        self.request = self.output_root = self.sidecar_oid = None
        self.phase,self.awaiting,self.put,self.cur = 'initial',None,None,None
        self.objects,self.steps,self.i = {},[],0
        self.status_i,self.sidecar_pending,self.refs,self.budget = 0,None,[],MAX_OUTPUT
        self.last_path = None
        authority.verify(grant,self.scope)

    # ---- retained-byte binding
    def _blob_nodes(self):
        return {n['path']:n for n in self.scope['recipe']['nodes'] if n['mode'] != '40000'}

    def _check_blob(self,path):
        node = self._blob_nodes()[path]
        raw = self.retained.get(path)
        if type(raw) is not bytes or len(raw) != node['length'] or digest(raw) != node['sha256']:
            raise Refused('retained bytes differ from recipe')
        return raw

    def _validate_retained(self):
        nodes = self._blob_nodes()
        if set(self.retained) != set(nodes): raise Refused('retained blob set differs from recipe')
        for path in nodes: self._check_blob(path)
        self.children = {}
        for n in self.scope['recipe']['nodes']:
            if n['path'] != 'args':
                parent,_,name = n['path'].rpartition('/')
                self.children.setdefault(parent,[]).append(name)
        for names in self.children.values(): names.sort(key=lambda s:s.encode())
        self.modes = {n['path']:n['mode'] for n in self.scope['recipe']['nodes']}
        def walk(path):
            if self.modes[path] == '40000':
                for name in self.children.get(path,()): walk(path+'/'+name)
                self.steps.append(('tree',path))
            else: self.steps.append(('blob',path))
        walk('args')
        self.steps.append(('tree','bundle'))

    def _construct(self,kind,path):
        """Serialize one expected wire object; record SHA-1 oid, SHA-256 ABI oid, wire digest."""
        if kind == 'blob':
            body = self._check_blob(path)
            sha1,sha256,wire = _wire('blob',body)
        else:
            if path == 'bundle': entries = [('40000','args')]; child = lambda n:'args'
            else:
                entries = [(self.modes[path+'/'+n],n) for n in self.children.get(path,())]
                child = lambda n:path+'/'+n
            entries.sort(key=lambda e:(e[1]+('/' if e[0] == '40000' else '')).encode())
            body = b''.join(m.encode()+b' '+n.encode()+b'\0'+bytes.fromhex(self.objects[child(n)]['oid']) for m,n in entries)
            abi_body = b''.join(m.encode()+b' '+n.encode()+b'\0'+bytes.fromhex(self.objects[child(n)]['abi']) for m,n in entries)
            sha1,_,wire = _wire('tree',body)
            sha256 = _wire('tree',abi_body)[1]
        if kind == 'blob': sha256 = _wire('blob',body)[1]
        self.objects[path] = dict(oid=sha1,abi=sha256,wire_sha256=digest(wire),kind=kind)
        return self.objects[path]

    def _submit_body(self):
        base = self.scope['recipe']['base']
        return ('{"base":{"type":%s,"value":%s},"bundle":%s,"salt":%s}'%(_json_str(base['type']),_json_str(base['value']),
                _json_str(self.objects['bundle']['oid']),_json_str(self.scope['recipe']['salt']))).encode()

    # ---- shape grammar (the whole check in unit-test mode)
    PAIRS = ((('HEAD','GET'),re.compile(r'/object/[0-9a-f]{40}')),(('POST',),re.compile(r'/object/')),(('POST',),re.compile(r'/submit')),
             (('GET',),re.compile(r'/version|/status/[0-9a-f]{40}(?:\?all=1)?|/run\?req=[0-9a-f]{40}&nocache=1')))

    def _shape_request(self,event):
        if (set(event) != {'phase','sequence','operation','method','path','body_sha256'}
                or not isinstance(event['method'],str) or not isinstance(event['path'],str)
                or (event['body_sha256'] is not None and not _hex(event['body_sha256']))):
            raise Refused('unsupported guarded request')
        for methods,pattern in self.PAIRS:
            if event['method'] in methods and pattern.fullmatch(event['path']):
                if (event['body_sha256'] is not None) != (event['method'] == 'POST'):
                    raise Refused('request body presence mismatch')
                return
        raise Refused('unsupported method/path pair')

    def approve(self,event):
        if (not isinstance(event,dict) or event.get('phase') != 'authorize'
                or type(event.get('sequence')) is not int or event['sequence'] != self.sequence+1):
            raise Refused('approval sequence mismatch')
        op = event.get('operation')
        keys = {'phase','sequence','operation'}
        if self.sequence == 0:
            if (op != 'initial' or set(event) != keys|{'endpoint','recipe'}
                    or event['endpoint'] != self.scope['recipient'] or event['recipe'] != self.scope['recipe']):
                raise Refused('retained recipe/endpoint mismatch')
            self.authority.verify(self.grant,self.scope)
            if self.strong:
                self._validate_retained()   # raw bytes vs recipe BEFORE the first ack
                self.phase = 'export'
        elif op == 'serialize':
            labels = {('tree:' if n['mode']=='40000' else 'blob:')+n['path'] for n in self.scope['recipe']['nodes']}|{'tree:bundle'}
            if set(event) != keys|{'label'} or event['label'] not in labels:
                raise Refused('ungranted serialization')
            self.authority.verify(self.grant,self.scope)
            if self.strong: self._expect(event)
        elif op == 'request':
            self._shape_request(event)
            self.authority.verify(self.grant,self.scope)
            if self.strong: self._expect(event)
        else: raise Refused('approval operation invalid')
        self.sequence += 1
        return dict(approve=self.sequence)

    # ---- ordered request graph
    def _req(self,event,method,path,body=None):
        if (event['operation'],event.get('method'),event.get('path'),event.get('body_sha256')) != ('request',method,path,body):
            raise Refused('request differs from expected graph step')

    def _expect(self,event):
        if self.awaiting: raise Refused('previous bridge row not yet observed')
        op = event['operation']
        if self.phase == 'export':
            if self.put == 'head':
                self._req(event,'HEAD','/object/'+self.cur['oid'])
                self.put = 'post'
                return
            if self.put == 'post':
                self.put = None   # HEAD status is not emitted: a POST now means a miss
                if op == 'request' and event['method'] == 'POST' and event['path'] == '/object/':
                    self._req(event,'POST','/object/',self.cur['wire_sha256'])
                    return
            if self.i < len(self.steps):
                kind,path = self.steps[self.i]
                if op != 'serialize' or event['label'] != kind+':'+path:
                    raise Refused('serialization out of order or repeated')
                self.i += 1
                self.cur = self._construct(kind,path)
                self.put = 'head'
                return
            self._req(event,'POST','/submit',digest(self._submit_body()))
            self.phase,self.awaiting = 'submitted',('submitted',)
            return
        if op != 'request': raise Refused('no private serialization after export')
        if self.phase == 'run':
            self._req(event,'GET','/run?req=%s&nocache=1'%self.request)
            self.phase,self.awaiting = 'executed',('executed',)
        elif self.phase == 'status':
            path = ['/version','/status/'+self.request,'/status/%s?all=1'%self.request][self.status_i]
            self._req(event,'GET',path)
            self.last_path,self.awaiting = path,('readback',)
        elif self.phase == 'objects':
            if self.sidecar_pending:
                self._req(event,'GET','/object/'+self.sidecar_pending)
                self.awaiting = ('sidecar',self.sidecar_pending)
                return
            if not self.refs: raise Refused('no known object left to read')
            oid,kind,depth,path = self.refs.pop()
            self._req(event,'GET','/object/'+oid)
            self.awaiting = ('object',oid,kind,depth,path)
        else: raise Refused('request not valid in this phase')

    def observe(self,row):
        """Validate EVERY non-authorize bridge row against the pending request."""
        if not isinstance(row,dict) or not isinstance(row.get('phase'),str): raise Refused('malformed bridge row')
        if not self.strong: raise Refused('observation requires retained blobs')
        wait = self.awaiting
        if not wait or _ROWS[wait[0]] != row['phase']: raise Refused('unexpected bridge row')
        getattr(self,'_row_'+wait[0].replace('-','_'))(row,wait)

    def _row_submitted(self,row,wait):
        r,o = self.scope['recipe'],self.objects
        want = dict(phase='submitted',request=row.get('request'),salt=r['salt'],args=o['args']['oid'],
                    state=o['args/state']['oid'],driver=o['args/driver']['oid'],worker=o['args/worker1']['oid'],
                    expected_image=r['image'],bundle=o['bundle']['oid'],pool='unforged')
        if row != want or not _oid(row['request']) or self.i != len(self.steps):
            raise Refused('submitted row differs from independently computed objects')
        self.request,self.awaiting,self.phase = row['request'],None,'run'

    def _row_executed(self,row,wait):
        if (set(row) != {'phase','request','result_oid','cache_hit','worker_image','server_confirmed_nocache'}
                or row['request'] != self.request or row['cache_hit'] is not False
                or row['worker_image'] != self.scope['recipe']['image']
                or row['server_confirmed_nocache'] is not True or not _oid(row['result_oid'])):
            raise Refused('executed row not an exact fresh miss for this request')
        self.output_root = row['result_oid']
        self.awaiting,self.phase,self.status_i = None,'status',0

    def _row_readback(self,row,wait):
        if (set(row) != {'phase','path','http_status','body'} or row['path'] != self.last_path
                or type(row['http_status']) is not int or not isinstance(row['body'],str)):
            raise Refused('readback differs from authorized request')
        self.status_i += 1
        self.awaiting = None
        if self.status_i == 3:
            self.phase = 'objects'
            if row['http_status'] == 200:
                value = strict_json(row['body'].encode())
                if isinstance(value,dict) and value.get('arg_tree') == self.request and isinstance(value.get('out_trace'),list):
                    traces = value['out_trace']
                    if len(traces) != 1 or not _oid(traces[0]): raise Refused('readback sidecar ambiguous or invalid')
                    self.sidecar_pending = traces[0]
            self.refs = [(self.output_root,'tree',0,'')]

    def _row_sidecar(self,row,wait):
        raw = row.get('raw_payload')
        if (set(row) != {'phase','request','oid','raw_payload'} or row['request'] != self.request or row['oid'] != wait[1]
                or not isinstance(raw,str) or _wire('blob',raw.encode())[0] != wait[1]):
            raise Refused('sidecar row differs from status-derived object')
        self.sidecar_oid,self.sidecar_pending,self.awaiting = wait[1],None,None

    def _row_object(self,row,wait):
        _,oid,kind,depth,path = wait
        if (set(row) != {'phase','oid','kind','hex'} or row['oid'] != oid or row['kind'] != kind or not isinstance(row['hex'],str)):
            raise Refused('object row differs from requested object')
        try: raw = bytes.fromhex(row['hex'])
        except ValueError: raise Refused('invalid raw object bytes') from None
        if _wire(kind,raw)[0] != oid: raise Refused('raw object hash mismatch')
        if len(raw) > self.budget: raise Refused('output budget exceeded')
        self.budget -= len(raw)
        if kind == 'blob':
            self.awaiting = ('abi-file',path,row['hex'])
            return
        if depth > 32: raise Refused('output depth exceeded')
        entries,offset,seen = [],0,set()
        while offset < len(raw):
            try:
                stop = raw.index(b'\0',offset)
                mode,name = raw[offset:stop].split(b' ',1)
                text = name.decode('utf-8')
            except (ValueError,UnicodeError): raise Refused('invalid tree encoding') from None
            child = raw[stop+1:stop+21]
            if len(child) != 20 or not _name(text) or text in seen or mode not in (b'40000',b'100644'):
                raise Refused('invalid output tree entry')
            seen.add(text)
            entries.append((mode,text,child.hex()))
            offset = stop+21
        for mode,text,child in reversed(entries):   # stack: first entry is read next (depth-first)
            self.refs.append((child,'tree' if mode == b'40000' else 'blob',depth+1,path+text+('/' if mode == b'40000' else '')))
        self.awaiting = None

    def _row_abi_file(self,row,wait):
        _,path,hexed = wait
        if set(row) != {'phase','path','mode','hex'} or row['path'] != path or row['mode'] != '100644' or row['hex'] != hexed:
            raise Refused('abi-file row differs from object just read')
        self.awaiting = None

    def finish(self):
        if (self.phase != 'objects' or self.awaiting or self.refs or self.sidecar_pending
                or self.output_root is None): raise Refused('guarded transcript incomplete')

def strict_json(raw):
    def pairs(items):
        result = {}
        for k,v in items:
            if k in result: raise Refused('duplicate protocol field')
            result[k] = v
        return result
    try: return json.loads(raw,object_pairs_hook=pairs)
    except (ValueError,UnicodeError) as error: raise Refused('malformed protocol JSON') from error

def run_guarded(command,session,*,bridge_sha256,env=None,timeout=240,on_event=None):
    """Execute only an independently pinned bridge; no keys in its environment.

    Mandatory: `session` was built with retained_blobs (strong verifier); otherwise
    refuses before spawning. No subprocess or file writes on initial grant denial.
    Every authorize row goes to session.approve, every other row to session.observe.
    Any Exception becomes a refusal; the child is ALWAYS killed and reaped in
    `finally`, also when a BaseException propagates. Retains stdout/stderr in bounded
    memory. on_event is for operator revocation testing, never candidate-controlled.
    Not a sandbox for a malicious bridge binary: the pinned bridge is trusted harness.
    """
    if not getattr(session,'strong',False): raise Refused('retained blob mapping required for guarded run')
    session.authority.verify(session.grant,session.scope)
    from pathlib import Path
    if digest(Path(command[0]).read_bytes()) != bridge_sha256:
        raise Refused('bridge differs from operator pin')
    outputs = {'stdout':bytearray(),'stderr':bytearray()}
    failure = None
    process = selector = None
    try:
        process = subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
        selector = selectors.DefaultSelector()
        selector.register(process.stdout,selectors.EVENT_READ,'stdout')
        selector.register(process.stderr,selectors.EVENT_READ,'stderr')
        pending = bytearray()
        deadline = time.monotonic()+timeout
        while selector.get_map():
            remaining = deadline-time.monotonic()
            if remaining <= 0: raise Refused('guarded bridge timeout')
            for key,_ in selector.select(min(remaining,1)):
                raw = os.read(key.fileobj.fileno(),65536)
                if not raw:
                    selector.unregister(key.fileobj)
                    continue
                outputs[key.data].extend(raw)
                if sum(map(len,outputs.values())) > 256*1024*1024:
                    raise Refused('guarded transcript too large')
                if key.data != 'stdout': continue
                pending.extend(raw)
                while b'\n' in pending:
                    line,_,tail = pending.partition(b'\n')
                    pending = bytearray(tail)
                    event = strict_json(line)
                    if not isinstance(event,dict): raise Refused('protocol row must be an object')
                    if on_event: on_event(event)
                    if event.get('phase') == 'authorize':
                        approval = session.approve(event)
                        process.stdin.write(canonical(approval)+b'\n')
                        process.stdin.flush()
                    else: session.observe(event)
        if pending: raise Refused('incomplete protocol row')
        code = process.wait(timeout=max(.1,deadline-time.monotonic()))
        if code != 0: failure = 'bridge refused'
        else: session.finish()
    except Exception as error:
        failure = str(error) or type(error).__name__
    finally:
        if process is not None:
            try:
                if process.poll() is None: process.kill()
            finally:
                try: process.wait()
                finally:
                    if selector is not None: selector.close()
                    for stream in (process.stdin,process.stdout,process.stderr):
                        try: stream.close()
                        except Exception: pass
    return dict(returncode=process.returncode if process else None,stdout=bytes(outputs['stdout']),stderr=bytes(outputs['stderr']),refusal=failure,approvals=session.sequence)
