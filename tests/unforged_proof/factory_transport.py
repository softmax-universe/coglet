"""Operator-owned synthetic factory transport using the existing guarded seam.

Configured in the service process, never by a candidate. Grant callbacks belong
to the independent operator authority. This does not grant device acquisition or
production promotion. STANDARD has no resolver input; its exact worker/driver/
base/bundle recipe and image replace no native-path resolver qualification.
"""
from dataclasses import dataclass
from pathlib import Path
import os
from coglet_gate import Refused, canonical, digest
from coglet_gate.hosted import step_salt
from guarded_preparation import Sources, prepare
from guarded_transport import scope_for, strict_json
from standard_step import require_fresh
from diagnostic_receipts import verify_isolation_sidecar
from wire_trees import verify_output

ROOT=Path(__file__).resolve().parents[2]

@dataclass(frozen=True)
class FactoryTransport:
    driver: Path
    bridge: Path
    driver_sha256: str
    bridge_sha256: str
    worker: bytes
    endpoint: str
    image: str
    sender: str
    evidence: Path
    source_authority: object
    recipe_authority: object
    source_grant: object
    recipe_grant: object
    authority_identity: str
    synthetic_transport_only: bool = False

    @property
    def identity(self):
        if self.synthetic_transport_only is not True:
            raise Refused('synthetic-only factory transport')
        if digest(self.driver.read_bytes()) != self.driver_sha256 or digest(self.bridge.read_bytes()) != self.bridge_sha256:
            raise Refused('operator factory binaries changed')
        # Pin every harness dependency, not just the injection module's filename.
        sources=[]
        for parent,pattern in [(Path(__file__).parent,'*.py'),(Path(__file__).parent/'bridge/src','*.rs')]:
            sources.extend((str(p.relative_to(ROOT)),digest(p.read_bytes())) for p in sorted(parent.glob(pattern)))
        return dict(implementation_sha256=digest(canonical(sources)),
            configuration_sha256=digest(canonical(dict(driver=self.driver_sha256,bridge=self.bridge_sha256,
                worker=digest(self.worker),sender=self.sender,endpoint=self.endpoint,image=self.image,
                authority=self.authority_identity,profile='synthetic-standard-guarded-factory-v1'))),
            worker_image=self.image,endpoint_identity=self.endpoint)

    def evaluate(self,work,request,archive,*,attempt):
        self.identity  # recheck current operator pins before every batch
        if archive.get('coglet') != request['binding']['coglet']:
            raise Refused('factory archive target mismatch')
        trace=canonical(archive)  # already-acquired service-local oracle, not wire serialization
        rows=[]
        for index,record in enumerate(archive['records']):
            salt=step_salt(request,index,attempt)
            snapshot=archive['initial_files'] if index==0 else archive['records'][index-1]['output_files']
            held=Sources(trace=trace,snapshot=canonical(snapshot),driver=self.driver.read_bytes(),worker=self.worker,
                step=index,sender=self.sender,recipient=self.endpoint,coglet=archive['coglet'],image=self.image,salt=salt)
            scope=held.scope()
            grant=self.source_grant(scope)
            prepared=prepare(held,self.source_authority,grant,scope)
            wire_scope=scope_for(prepared.recipe,sender=self.sender,recipient=self.endpoint,coglet=held.coglet)
            wire_grant=self.recipe_grant(wire_scope)
            result=prepared.execute(parent=work,name='step-'+str(index),recipe_authority=self.recipe_authority,
                recipe_grant=wire_grant,recipe_scope=wire_scope,driver=self.driver,bridge=self.bridge,
                bridge_sha256=self.bridge_sha256,synthetic_transport_only=True)
            # Preserve actual failures and transcripts as well as successes. All
            # synthetic-only; the operator chooses the durable evidence directory.
            out=self.evidence/salt
            out.mkdir(parents=True,exist_ok=False,mode=0o700)
            def redacted(raw):
                for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
                    if os.environ.get(name):raw=raw.replace(os.environ[name].encode(),b'[REDACTED]')
                return raw
            raw=redacted(result.pop('stdout'))
            (out/'bridge.jsonl').write_bytes(raw)
            (out/'bridge-error.txt').write_bytes(redacted(result.pop('stderr')))
            (out/'exit.json').write_bytes(canonical(result))
            (out/'scope.json').write_bytes(canonical(wire_scope))
            if result['refusal']:raise Refused('guarded factory execution refused')
            transcript=[strict_json(line) for line in raw.splitlines()]
            def one(phase):
                found=[r for r in transcript if r.get('phase')==phase]
                if len(found)!=1:raise Refused('missing/ambiguous factory '+phase)
                return found[0]
            submitted,executed,sidecar=one('submitted'),one('executed'),one('sidecar-readback')
            require_fresh(executed,request=submitted['request'],image=self.image)
            verify_isolation_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=submitted['request'],salt=salt)
            tree=verify_output(transcript,executed['result_oid'])
            if tree != record['output_hash']:raise Refused('factory hosted whole tree mismatch')
            row=dict(index=index,salt=salt,request_oid=submitted['request'],tree=tree,
                     worker_image=self.image,transcript_sha256=digest(raw))
            if index == 0:
                self.source_authority.verify(grant, scope)
                (out/'native-oracle.json').write_bytes(held.trace)
            (out/'result.json').write_bytes(canonical(row))
            rows.append(row)
        return rows
