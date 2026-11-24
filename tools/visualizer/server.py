"""Read-only loopback view of an explicitly selected operator ABI record export.

This is not a hub debug transport, runtime controller, WAL reader, authenticator
for exported records, or an independent replay verifier.
"""
import argparse
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import importlib.util
import stat
from urllib.parse import urlsplit

ASSETS = Path(__file__).with_name("web")
MAX_BYTES = 16 * 1024 * 1024
MAX_RECORD_BYTES = 1024 * 1024
MAX_HISTORY = 128


def project(record):
    """Validate the display subset only; never claim complete ABI validation."""
    if not isinstance(record, dict) or record.get("version") != "0.1":
        raise ValueError("Invalid record version")
    for field, pattern in [("coglet", r"coglet:[0-9a-f]{64}"), ("trace_id", r"[0-9a-f]{32}"),
                           ("output_hash", r"[0-9a-f]{64}"), ("record_hash", r"[0-9a-f]{64}")]:
        if not isinstance(record.get(field), str) or not re.fullmatch(pattern, record[field]):
            raise ValueError("Invalid record identity/hash")
    for field in ["step", "logical_time"]:
        if type(record.get(field)) is not int or not 0 <= record[field] <= 2**64 - 1:
            raise ValueError("Invalid unsigned record counter")
    if record.get("status") not in {"committed", "aborted"}:
        raise ValueError("Invalid record status")
    events, files = record.get("events"), record.get("output_files")
    if not isinstance(events, list) or len(events) > 4096 or not isinstance(files, dict):
        raise ValueError("Invalid events/files")
    for event in events:
        if not isinstance(event, dict) or not all(isinstance(event.get(k), str) for k in ["kind", "source"]):
            raise ValueError("Invalid event")
    output = {}
    for path, value in files.items():
        if not isinstance(path, str) or not isinstance(value, str) or not re.fullmatch(r"(?:[0-9a-f]{2})*", value):
            raise ValueError("Invalid exported file bytes")
        if path.startswith("/data/"):
            output[path] = value
    fields = ["version", "coglet", "trace_id", "step", "logical_time", "status", "output_hash", "record_hash"]
    return {**{key: record[key] for key in fields}, "events": events, "output_files": output}


class RecordStream:
    def __init__(self, path, *, source_format="jsonl"):
        self.path = Path(path)
        if source_format not in {"jsonl", "trace"}:
            raise ValueError("Unknown export format")
        self.source_format = source_format

    def snapshot(self):
        # Pin the actual descriptor and refuse special files before reading.
        fd = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as source:
            before = os.fstat(source.fileno())
            if not stat.S_ISREG(before.st_mode):
                raise ValueError("Export must be a regular file")
            if before.st_size > MAX_BYTES:
                raise ValueError("Export size limit; rotate the selected export")
            raw = source.read(MAX_BYTES + 1)
            if len(raw) > MAX_BYTES:
                raise ValueError("Export size limit")
        records = deque(maxlen=MAX_HISTORY)
        def decode_json(raw):
            def nonfinite(value):
                raise ValueError("Nonfinite JSON")
            return json.loads(raw, parse_constant=nonfinite)
        if self.source_format == "trace":
            trace = decode_json(raw)
            if not isinstance(trace, dict) or not isinstance(trace.get("records"), list):
                raise ValueError("Missing trace records")
            values = trace["records"]
        else:
            lines = [line for line in raw.split(b"\n")[:-1] if line]
            if any(len(line) > MAX_RECORD_BYTES for line in lines):
                raise ValueError("Record size limit")
            values = map(decode_json, lines)
        count = 0
        for value in values:
            records.append(project(value))
            count += 1
        return {"state": "observing" if records else "waiting", "records": list(records),
                "omitted": max(0, count - MAX_HISTORY), "modified_at": before.st_mtime}


def browser_values(value):
    """Lossless display JSON: JavaScript cannot represent all ABI uint64s."""
    if type(value) is int and abs(value) > 2**53 - 1:
        return str(value)
    if isinstance(value, dict):
        return {key: browser_values(item) for key, item in value.items()}
    if isinstance(value, list):
        return [browser_values(item) for item in value]
    return value


class LocalServer(ThreadingHTTPServer):
    url: str
    # The TCP backlog bounds pending requests; private data still requires token.
    request_queue_size = 8

    def server_close(self):
        if getattr(self, 'native', None) is not None:
            self.native.close()
        super().server_close()


def make_server(path, *, port=0, world_path=None, source_format="jsonl", native_trace=None, native_helper=None):
    if world_path is not None and not world_path.startswith("/data/"):
        raise ValueError("World path must name an exported /data file")
    stream = RecordStream(path, source_format=source_format)
    token = secrets.token_urlsafe(32)
    native = None
    if (native_trace is None) != (native_helper is None):
        raise ValueError('Select both native trace and prebuilt helper')
    if native_trace is not None:
        spec = importlib.util.spec_from_file_location('visualizer_native', Path(__file__).with_name('native_replay.py'))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        native = module.NativeReplay(native_trace, native_helper)

    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(5)

        def log_message(self, format, *args):
            pass  # Never log capability URLs or private records.

        def end_headers(self):
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
            super().end_headers()

        def do_POST(self):
            origin = f"http://127.0.0.1:{self.server.server_port}"
            if (self.headers.get("Host") != origin.removeprefix('http://') or
                    self.headers.get("Origin", origin) != origin or
                    not secrets.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + token)):
                self.send_error(403)
                return
            route = urlsplit(self.path).path
            if route not in {'/api/simulate', '/api/native'}:
                self.send_error(404)
                return
            try:
                length = int(self.headers.get('Content-Length', '-1'))
                if self.headers.get('Transfer-Encoding') or not 0 < length <= (65536 if route == '/api/native' else MAX_BYTES):
                    raise ValueError('Upload size limit')
                raw = self.rfile.read(length)
                if len(raw) != length:
                    raise ValueError('Incomplete upload')
                def pairs(items):
                    result = {}
                    for key, value in items:
                        if key in result:
                            raise ValueError('Duplicate JSON key')
                        result[key] = value
                    return result
                def nonfinite(value):
                    raise ValueError('Nonfinite JSON')
                payload = json.loads(raw, object_pairs_hook=pairs, parse_constant=nonfinite)
                if route == '/api/native':
                    if native is None:
                        raise ValueError('No native source selected at startup')
                    result = native.replay(payload)
                    records = [project(r) for r in result.pop('records')]
                    body = json.dumps(browser_values(dict(state='native', records=records,
                                      native=result, world_path=None))).encode()
                else:
                    if not isinstance(payload, dict) or set(payload) - {'archive', 'actions'}:
                        raise ValueError('Invalid simulation request')
                    spec = importlib.util.spec_from_file_location('visualizer_simulation', Path(__file__).with_name('simulation.py'))
                    module = importlib.util.module_from_spec(spec)
                    spec.loader.exec_module(module)
                    result = module.replay(payload.get('archive'), payload.get('actions'))
                    records = [project(r) for r in payload['archive']['records']]
                    body = json.dumps(browser_values(dict(state='simulated', records=records,
                                      simulation=result, world_path=None))).encode()
            except (ValueError, TypeError, KeyError, RecursionError, OSError):
                self.send_error(400, 'Invalid or incomplete simulation archive; previous result is not current')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'application/json; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def do_GET(self):
            origin = f"http://127.0.0.1:{self.server.server_port}"
            if self.headers.get("Host") != f"127.0.0.1:{self.server.server_port}" or self.headers.get("Origin", origin) != origin:
                self.send_error(403)
                return
            route = urlsplit(self.path).path
            if route == "/api/snapshot":
                if not secrets.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token):
                    self.send_error(403)
                    return
                try:
                    data = stream.snapshot()
                except (OSError, ValueError, RecursionError):
                    data = {"state": "error", "records": [], "error": "Selected export is missing, invalid, or over its size limit"}
                data["world_path"] = world_path
                body, content_type = json.dumps(browser_values(data)).encode(), "application/json"
            elif route in {"/", "/app.js", "/worlds.js", "/style.css"}:
                filename, content_type = {
                    "/": ("index.html", "text/html"),
                    "/worlds.js": ("worlds.js", "text/javascript"),
                    "/app.js": ("app.js", "text/javascript"),
                    "/style.css": ("style.css", "text/css"),
                }[route]
                body = (ASSETS / filename).read_bytes()
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", content_type + "; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = LocalServer(("127.0.0.1", port), Handler)
    server.native = native
    server.url = f"http://127.0.0.1:{server.server_port}/#{token}"
    return server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sources = parser.add_mutually_exclusive_group(required=True)
    sources.add_argument("--records", type=Path, help="Newline-terminated JSONL step record export")
    sources.add_argument("--trace", type=Path, help="Atomically published ABI trace JSON export")
    parser.add_argument("--port", type=int, default=8088)
    parser.add_argument("--world-path", help="Explicit /data path containing a world observation")
    parser.add_argument("--native-trace", type=Path, help="Complete local canonical archive for offline native replay")
    parser.add_argument("--native-helper", type=Path, help="Fixed prebuilt native replay executable (Linux)")
    args = parser.parse_args()
    server = make_server(args.records or args.trace, port=args.port, world_path=args.world_path,
                         source_format="trace" if args.trace else "jsonl",
                         native_trace=args.native_trace, native_helper=args.native_helper)
    print(server.url, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
