"""Pre-I/O model reply completeness for independently native-verified archives.

Not an archive authenticity verifier. Call native record validation first; this
adds the stronger hosted-gate requirement that no recorded model request remains
pending. It neither calls a provider nor repairs missing outcomes.
"""
import json


def require_model_reply_closure(trace):
    pending = {}
    seen = set()
    replies = 0
    for request in trace.get('initial_llm_requests', []):
        key = request['id']
        if key in seen:
            raise ValueError('duplicate recorded model request')
        seen.add(key)
        pending[key] = request
    records = list(trace['records'])
    if trace.get('trailing_boundary') is not None:
        records.append(trace['trailing_boundary'])
    for record in records:
        for change in record['boundary']:
            if change['path'] != '/data/inbox.jsonl':
                continue
            before = bytes.fromhex(change['before'] or '')
            after = bytes.fromhex(change['after'] or '')
            if not after.startswith(before):
                raise ValueError('nonappend recorded inbox')
            for line in after[len(before):].splitlines():
                message = json.loads(line)
                event = message['event']
                if event['kind'] != 'llm.reply':
                    continue
                payload = event['payload']
                request = pending.pop(payload['request_id'], None)
                if request is None:
                    raise ValueError('missing request or duplicate recorded model reply')
                if event['correlation_id'] != request['correlation_id']:
                    raise ValueError('recorded model correlation differs')
                if payload['status'] not in ('succeeded', 'failed'):
                    raise ValueError('nonterminal recorded model reply')
                replies += 1
        if record.get('status') == 'committed':
            for request in record['llm_requests']:
                key = request['id']
                if key in seen:
                    raise ValueError('duplicate recorded model request')
                seen.add(key)
                pending[key] = request
    if pending:
        raise ValueError('missing recorded model reply')
    return {'requests': len(seen), 'replies': replies}
