"""Trusted operator hosted evaluator seam; no transport or production authority.

The injected evaluator is INSIDE the operator trust boundary, like native tools.
It must authenticate and verify the complete transport transcript, not accept a
factory report. This service-side checker additionally binds its closed result to
the retained native archive and challenge. It is not remote attestation by itself.
"""
from . import Refused, canonical, digest, hex_value


def step_salt(request, index, attempt):
    if not hex_value(attempt, (64,)):
        raise Refused('operator invocation nonce required')
    if type(index) is not int or not 0 <= index < 1024:
        raise Refused('bounded hosted step index required')
    return digest(b'coglet-hosted-gate-step-v1\0'+canonical(request)+b'\0'+attempt.encode('ascii')+b'\0'+str(index).encode('ascii'))


def identity(evaluator):
    try:
        value = evaluator.identity
        if (not isinstance(value, dict)
                or set(value) != {'implementation_sha256','configuration_sha256','worker_image','endpoint_identity'}
                or any(not hex_value(value[k],(64,)) for k in ('implementation_sha256','configuration_sha256','endpoint_identity'))
                or not isinstance(value['worker_image'],str)
                or not value['worker_image'].startswith('native@sha256:')
                or not hex_value(value['worker_image'][14:],(64,))):
            raise Refused('closed operator hosted identities required')
        return {k:v for k,v in value.items()}
    except (AttributeError,TypeError) as error:
        raise Refused('operator hosted evaluator unavailable') from error


def validate_steps(steps, archive, request, pinned, attempt):
    """Check exact ordered coverage; transcript authentication is the runner's job."""
    try:
        records = archive['records']
        if (not isinstance(archive,dict) or archive['coglet'] != request['binding']['coglet']
                or not isinstance(records,list) or not 1 <= len(records) <= 1024
                or not isinstance(steps,list) or len(steps) != len(records)):
            raise Refused('complete hosted step coverage required')
        seen = set()
        for index,(step,record) in enumerate(zip(steps,records,strict=True)):
            if (not isinstance(step,dict)
                    or set(step) != {'index','salt','request_oid','tree','worker_image','transcript_sha256'}
                    or type(step['index']) is not int or step['index'] != index
                    or step['salt'] != step_salt(request,index,attempt)
                    or not hex_value(step['request_oid'],(40,)) or step['request_oid'] in seen
                    or not hex_value(step['tree'],(64,)) or step['tree'] != record['output_hash']
                    or step['worker_image'] != pinned['worker_image']
                    or not hex_value(step['transcript_sha256'],(64,))):
                raise Refused('hosted step binding or transcript identity mismatch')
            seen.add(step['request_oid'])
        return dict(version=1,identity=pinned,attempt=attempt,steps=steps)
    except (KeyError,TypeError,ValueError) as error:
        if isinstance(error,Refused): raise
        raise Refused('malformed hosted step evidence') from error
