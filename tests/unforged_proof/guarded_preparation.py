"""Two operator grants around initial preparation and guarded STANDARD staging.

Only consumes already-held immutable source bytes from a trusted, native-validated
recording/snapshot. It does NOT authenticate a recording, fetch a private source,
issue grants, construct keys, or authorize a device read. The first existing
ExportAuthority grant binds every retained source and operator selection BEFORE
JSON decode/private payload serialization. The second existing RecipeAuthority
grant binds the resulting complete transport recipe BEFORE any disk staging.
Both remain live at every staging write and every transport approval. Revocation
stops the next crossing, not earlier disclosure. Synthetic testbed only; no PASS.
"""
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
from types import MappingProxyType, SimpleNamespace

from coglet_gate import Refused, canonical
from guarded_transport import ApprovalSession, recipe_for, strict_json, _scope
from hosted_export import _scope as export_scope
from prefix_inputs import step_payload
from recorded_closure import require_model_reply_closure
from standard_step import state_files, snapshot_tree


def _blob(raw):
    # Hash held bytes incrementally; do not materialize a private wire object.
    h = hashlib.sha256()
    h.update(b'blob '+str(len(raw)).encode()+b'\0')
    h.update(raw)
    return dict(oid=h.hexdigest(),kind='blob',length=len(raw))


@dataclass(frozen=True)
class Sources:
    trace: bytes
    snapshot: bytes
    driver: bytes
    worker: bytes
    step: int
    sender: str
    recipient: str
    coglet: str
    image: str
    salt: str

    def __post_init__(self):
        for name,limit in [('trace',64),('snapshot',64),('driver',256),('worker',1)]:
            raw = getattr(self,name)
            if type(raw) is not bytes or len(raw) > limit*1024*1024:
                raise Refused('bounded immutable sources required')
        if type(self.step) is not int or self.step < 0:
            raise Refused('operator step required')
        # Validate public selection and source scope without decoding private JSON.
        recipe_for({'selection':b''},[],driver=self.driver,worker=self.worker,
                   salt=self.salt,image=self.image)
        self.scope()

    def arguments(self):
        return dict(vars(self))

    def scope(self):
        # Public selection metadata is itself a real retained blob descriptor,
        # not an invented tree OID. Role names bind otherwise swappable sources.
        descriptors = {n:_blob(getattr(self,n)) for n in ('trace','snapshot','driver','worker')}
        binding = canonical(dict(version=1,profile='synthetic-standard-preparation',
            step=self.step,sender=self.sender,recipient=self.recipient,coglet=self.coglet,
            image=self.image,salt=self.salt,sources=descriptors))
        objects = {d['oid']:d for d in [*descriptors.values(),_blob(binding)]}
        return export_scope(dict(sender=self.sender,recipient=self.recipient,coglet=self.coglet,
            objects=list(objects.values()),operations=['export','read']))


class _Both:
    def __init__(self,prepared,authority):
        self.prepared,self.authority = prepared,authority

    def verify(self,grant,scope):
        p = self.prepared
        p.source_authority.verify(p.source_grant,json.loads(p.source_scope))
        self.authority.verify(grant,scope)


@dataclass(frozen=True)
class Prepared:
    sources: Sources
    source_authority: object
    source_grant: str
    source_scope: bytes
    _files: tuple
    _dirs: tuple
    _recipe: bytes

    @property
    def files(self):
        return MappingProxyType(dict(self._files))

    @property
    def recipe(self):
        return json.loads(self._recipe)

    @contextmanager
    def stage(self,parent,name,authority,grant,scope):
        """Fresh 0700 directory, dirfd/no-follow writes, cleanup on every exit.

        Parent must be operator-owned, not writable by candidate processes. No
        grant can make an untrusted same-UID coordinator safe. Every directory
        component is opened without following symlinks. Keep this context alive
        through run_guarded; path references the retained root fd on Linux. The
        bridge's initial recipe check binds the staged bytes before any upload.
        """
        if not isinstance(name,str) or name in ('','.','..') or '/' in name or '\\' in name or '\0' in name:
            raise Refused('single staging name required')
        scope = _scope(scope)
        expected = dict(version=1,sender=self.sources.sender,recipient=self.sources.recipient,
            coglet=self.sources.coglet,operations=['export','read'],recipe=self.recipe)
        if scope != expected: raise Refused('staging recipe scope differs')
        both = _Both(self,authority)
        def check(): both.verify(grant,scope)
        check()  # before ANY staging syscall
        fds,created_files,created_dirs = {},[],[]
        parent_fd = root_fd = None
        made_root = False
        try:
            absolute = Path(parent).absolute()
            fd = os.open('/',os.O_RDONLY|os.O_DIRECTORY)
            try:
                for part in absolute.parts[1:]:
                    if part in ('.','..'): raise Refused('unsafe staging parent')
                    next_fd = os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd)
                    os.close(fd)
                    fd = next_fd
                parent_fd,fd = fd,None
            finally:
                if fd is not None: os.close(fd)
            check()
            os.mkdir(name,mode=0o700,dir_fd=parent_fd)
            made_root = True
            root_fd = os.open(name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=parent_fd)
            fds[''] = root_fd
            for directory in self._dirs:
                parent_path,_,leaf = directory.rpartition('/')
                check()
                os.mkdir(leaf,mode=0o700,dir_fd=fds[parent_path])
                created_dirs.append((fds[parent_path],leaf))
                fds[directory] = os.open(leaf,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fds[parent_path])
            for path,raw in self._files:
                parent_path,_,leaf = path.rpartition('/')
                check()
                fd = os.open(leaf,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=fds[parent_path])
                created_files.append((fds[parent_path],leaf))
                try:
                    offset = 0
                    while offset < len(raw):
                        check()
                        count = os.write(fd,raw[offset:])
                        if count <= 0: raise Refused('staging write failed')
                        offset += count
                finally: os.close(fd)
            check()
            retained = {'args/state/'+p:raw for p,raw in self._files}
            retained.update({'args/driver':self.sources.driver,'args/worker1':self.sources.worker,
                'args/salt':self.sources.salt.encode(),'args/required-native-worker-image':self.sources.image.encode(),
                'args/required-pool':b'unforged'})
            session = ApprovalSession(both,grant,scope,retained_blobs=retained)
            yield SimpleNamespace(path=f'/proc/{os.getpid()}/fd/{root_fd}',session=session)
        except OSError:
            raise Refused('staging filesystem refused') from None
        finally:
            # Cleanup uses retained fds, never follows candidate-controlled paths,
            # and does not require a grant that may already have been revoked.
            for fd,leaf in reversed(created_files): os.unlink(leaf,dir_fd=fd)
            for fd,leaf in reversed(created_dirs): os.rmdir(leaf,dir_fd=fd)
            for fd in reversed(list(fds.values())): os.close(fd)
            if made_root: os.rmdir(name,dir_fd=parent_fd)
            if parent_fd is not None: os.close(parent_fd)


def prepare(sources,authority,grant,scope):
    expected = sources.scope()
    if export_scope(scope) != expected: raise Refused('initial preparation source scope differs')
    authority.verify(grant,expected)  # before decoding or serializing private inputs
    trace,snapshot = strict_json(sources.trace),strict_json(sources.snapshot)
    require_model_reply_closure(trace)
    if not isinstance(trace,dict) or trace.get('coglet') != sources.coglet:
        raise Refused('operator target differs from recording')
    try:
        payload = step_payload(trace,sources.step)
        authority.verify(grant,expected)
        files = state_files(snapshot,payload)
        prior = {p:raw for p,raw in files.items() if p != 'proof-input.json'}
        if snapshot_tree(prior) != payload['prefix']['final_hash']:
            raise Refused('recorded prior tree differs')
        dirs = set()
        for path in files:
            parts = PurePosixPath(path).parts
            if len(parts) > 32: raise Refused('state depth exceeded')
            for i in range(1,len(parts)): dirs.add('/'.join(parts[:i]))
        if len(files)+len(dirs) > 99990 or sum(map(len,files.values())) > 64*1024*1024:
            raise Refused('state budget exceeded')
        dirs = tuple(sorted(dirs,key=lambda p:(p.count('/'),p)))
        authority.verify(grant,expected)
        recipe = recipe_for(files,dirs,driver=sources.driver,worker=sources.worker,
            salt=sources.salt,image=sources.image)
    except (KeyError,TypeError,ValueError) as error:
        if isinstance(error,Refused): raise
        raise Refused('invalid recorded preparation sources') from None
    return Prepared(sources,authority,grant,canonical(expected),tuple(sorted(files.items())),dirs,canonical(recipe))
