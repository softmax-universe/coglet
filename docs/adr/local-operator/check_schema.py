"""ADR message-shape checks only; no socket or deployment implementation."""
import copy
import json
from pathlib import Path

from jsonschema import Draft202012Validator
from referencing import Registry, Resource

ROOT = Path(__file__).resolve().parents[3]
SCHEMA = Path(__file__).with_name("operator.schema.json")
paths = [SCHEMA, ROOT / "spec/messages/common.schema.json", ROOT / "spec/messages/hub.schema.json"]
registry = Registry()
for path in paths:
    schema = json.loads(path.read_text())
    Draft202012Validator.check_schema(schema)
    registry = registry.with_resource(schema["$id"], Resource.from_contents(schema))
validator = Draft202012Validator(json.loads(SCHEMA.read_text()), registry=registry)
count = {"valid": 0, "invalid": 0}


def check(value, valid):
    errors = list(validator.iter_errors(value))
    assert bool(errors) != valid, (value, [e.message for e in errors])
    count["valid" if valid else "invalid"] += 1


ID = "a" * 32
NODE = "b" * 64
TARGET = "coglet:" + "c" * 64
error = {"code": "conflict", "message": "changed intent", "retryable": False, "details": {}}
grant = {"peer": NODE, "method": "hub.start", "target": TARGET, "generation": 1, "expires_unix_s": 1900000000}
build = {"device": NODE, "target": TARGET, "source_commit": "d" * 40, "build_profile": "locked", "delegation_id": "initial"}
lifecycle = {"device": NODE, "target": TARGET}
params = {
    "deploy": build, "push": build, "stop": lifecycle, "status": lifecycle,
    "grant.provision": grant,
    "grant.revoke": {k: v for k, v in grant.items() if k != "expires_unix_s"},
    "enrollment.window": {"duration_ms": 30000},
    "enrollment.invite": {"window_id": ID, "expected_device": NODE, "secret_sha256": "e" * 64, "ttl_ms": 1000, "grants": [grant]},
}
for op, value in params.items():
    request = {"version": "local-1", "operation_id": ID, "op": op, "timeout_ms": 1000, "params": value}
    check(request, True)
    for field, replacement in [("actor", TARGET), ("op", "hub.deploy"), ("operation_id", ID + "\n"), ("params", {}), ("timeout_ms", 0)]:
        bad = copy.deepcopy(request)
        bad[field] = replacement
        check(bad, False)
    bad = copy.deepcopy(request)
    bad["params"]["unknown"] = True
    check(bad, False)
    for field in value:
        bad = copy.deepcopy(request)
        bad["params"].pop(field)
        check(bad, False)
    for state in ["queued", "applied", "cancelled", "failed"]:
        receipt = {"operation_id": ID, "state": state, "step": 1 if state == "applied" else None, "error": error if state == "failed" else None}
        result = None
        if state == "applied":
            if op == "status":
                result = {"state": "stopped", "live_commit": None, "step": 0, "pending": 0, "error": None}
            elif op in ["enrollment.window", "enrollment.invite"]:
                result = ID
        response = {"version": "local-1", "operation_id": ID, "op": op, "receipt": receipt, "result": result, "error": None}
        check(response, True)
        bad = copy.deepcopy(response)
        bad["result"] = {"unrecognized": True}
        check(bad, False)
        bad = copy.deepcopy(response)
        bad["receipt"]["step"] = -1
        check(bad, False)
    check({"version": "local-1", "operation_id": ID, "op": op, "receipt": None, "result": None, "error": error}, True)
print(f"ADR-0018 schema: {count['valid']} valid and {count['invalid']} invalid controls passed")
