"""Best-effort bounded failure evidence, never transport recovery."""
import json




def retain_failure(error, tmp_path, record):
    """Read via a separate file description: child stderr's offset is shared."""
    try:
        log = tmp_path / 'daemon.log'
        with log.open('rb') as stream:
            size = stream.seek(0, 2)
            stream.seek(max(0, size - 8192))
            tail = stream.read(8192)
        record.update(stderr_path=str(log), stderr_bytes=size,
                      stderr_truncated=size > 8192,
                      stderr_tail=tail.decode('utf-8', errors='replace'))
        pending = tmp_path / 'native-failure.json.pending'
        pending.write_text(json.dumps(record, sort_keys=True), encoding='utf-8')
        pending.replace(tmp_path / 'native-failure.json')
        note = 'native failure: ' + json.dumps(record, sort_keys=True)
    except Exception as diagnostic_error:
        note = f'native failure diagnostic_error: {diagnostic_error!r}'
    try:
        error.add_note(note)
    except Exception:
        pass
