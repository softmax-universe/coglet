"""Independent assertions on original reopened-WAL evidence from the real peer.

No replies are generated here. The harness checks wire expectations separately;
its teardown calls these stronger persistence/nondisclosure assertions and fails
closed on missing evidence. Only explicit fixture clock fields may be ignored.
"""
import copy
import json
from pathlib import Path

OTHER = "8" * 32
INBOX_OP = f"{15:032x}"
MESSAGE = "f" * 32
HOLD_OP = f"{6:032x}"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def files(state):
    staged = state["staged_boundary"]
    return (staged["runtime"] if staged else state["runtime"])["files"]


def stable(state):
    result = copy.deepcopy(state)
    result.pop("clock_anchor", None)
    result["runtime"].pop("time", None)
    if result["staged_boundary"]:
        result["staged_boundary"]["runtime"].pop("time", None)
    return result


def no_effect(d):
    require(stable(d["state"]) == stable(d["baseline"]), "unexpected WAL effect")
    require(d["records"] == d["baseline_records"], "unexpected guest journal effect")


def error_only(reply, code):
    require(set(reply) == {"type", "correlation_id", "error"}, "negative disclosed result/metadata")
    require(reply["type"] == "error" and reply["error"] == {
        "code": code, "message": code, "retryable": code == "resource_exhausted",
    }, "unexpected negative/error disclosure")


def operation(state, request):
    id_ = request["correlation_id"]
    saved = state["receiver"]["operations"][id_]
    # Timeout is a reply allowance, not the retained intent; frozen requests in
    # this suite do not vary it, but scope and every other byte remain pinned.
    require(saved["request"] == request, "retained request changed")
    ctx = request.get("context", request["params"])
    scope = {"actor":ctx["actor"], "method":request["method"]}
    if ctx.get("ticket") is not None:
        scope["ticket"] = ctx["ticket"]
    require(saved["scope"] == scope, "receipt scope changed")
    require(saved["receipt"]["operation_id"] == id_, "receipt ID changed")
    return saved


def terminal_audit(d, request, outcome):
    id_ = request["correlation_id"]
    old = d["baseline"]["receiver"]["audit"]
    audit = d["state"]["receiver"]["audit"]
    require(audit[:len(old)] == old, "historical audit changed")
    rows = [a for a in audit[len(old):] if a["operation_id"] == id_]
    require([a["outcome"] for a in rows] == ["queued", outcome], "not exactly one admission and terminal audit")
    ctx = request.get("context", request["params"])
    for row in rows:
        require(row["actor"] == ctx["actor"] and row["ticket"] == ctx.get("ticket")
                and row["method"] == request["method"] and row["target"] == request["target"],
                "audit lost authenticated origin/binding")
    return rows[-1]


def validate(d):
    case = d["case"]
    b, s = d["baseline"], d["state"]
    records = d["records"]
    obs = d["observations"]
    root = Path(__file__).resolve().parents[2]
    frozen = json.loads((root / "conformance/hub" / (case + ".json")).read_text())
    require([r for r, _ in obs] == [step["send"] for step in frozen["steps"] if "send" in step],
            "evidence does not contain the exact complete frozen requests")
    require(len(d["baseline_records"]) == 1 and d["baseline_records"][0]["step"] == 0
            and d["baseline_records"][0]["status"] == "committed", "not an executed step-zero baseline")
    require(b["router"]["queue"] == [] and b["receiver"]["pending"] == []
            and "/data/inbox.jsonl" not in files(b), "dirty default fixture")
    require(records[:1] == d["baseline_records"], "original native step zero changed")
    require(s["runtime"]["unresolved_attempt"] is None, "guest did not actually finish")
    for key in ("dispatches", "dispatch_order", "private_events", "heard_pending", "supervisor_reads",
                "surface_epochs", "surface_sequences", "speech_temporal", "catalog_pin", "completed_segments"):
        require(s[key] == b[key], f"unexpected {key} effect")
    for request, reply in obs:
        require(reply["correlation_id"] == request["correlation_id"], "reply rebound")

    if case.startswith(("read-", "status-", "fetch-")) and case != "read-grant-is-not-write":
        no_effect(d)
        request, reply = obs[-1]
        if case.endswith("scoped-allowed"):
            require(request["context"] == {"actor":request["target"], "ticket":"6"*32}, "positive scope not exact")
            if case.startswith("read-"):
                raw = bytes.fromhex(files(s)[request["params"]["path"]])
                require(reply["type"] == "reply" and reply["result"]["text"].encode() == raw
                        and reply["result"]["step"] == 0 and reply["result"]["eof"] is True
                        and reply["result"]["next_offset"] == len(raw), "not committed scoped read")
                # Use the frozen ABI's independently implemented deterministic CBOR.
                import sys
                sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "conformance"))
                from wire import encode
                import hashlib
                require(reply["result"]["snapshot_hash"] == hashlib.sha256(encode(files(s))).hexdigest(),
                        "snapshot hash is not actual committed materialized files")
            elif case.startswith("status-"):
                require(reply["result"] == {"state":"stopped", "step":0, "pending":0, "error":None,
                    "live_commit":s["managed"]["live_commit"]}, "not exact committed scoped status")
            else:
                root = Path(__file__).resolve().parents[2]
                fixture = next(r for r in json.loads((root / "crates/coglet-runtime/tests/data/hub0/fetch.json").read_text())
                               if r["ref"] == request["params"]["ref"])["history"][-1]
                require(reply == {"type":"transfer", "correlation_id":request["correlation_id"],
                    "size":len(bytes.fromhex(fixture["bundle_hex"])), "sha256":fixture["sha256"],
                    "update":fixture["update"]}, "fetch metadata not exact granted snapshot")
        else:
            error_only(reply, "permission_denied")
        return

    if case in {"cross-owner-receipt", "unknown-receipt"}:
        no_effect(d)
        error_only(obs[-1][1], "not_found")
        if case == "cross-owner-receipt":
            saved = s["receiver"]["operations"][OTHER]
            require(saved == b["receiver"]["operations"][OTHER]
                    and saved["receipt"]["state"] == "applied"
                    and saved["scope"]["actor"] != obs[-1][0]["context"]["actor"]
                    and bytes.fromhex(files(s)["/data/notes/other"]) == b"owned",
                    "foreign original receipt/effect lost")
        else:
            requested = obs[-1][0]["params"]["operation_id"]
            require(requested not in s["receiver"]["operations"]
                    and requested not in s["receiver"]["cancellations"], "unknown ID unexpectedly exists")
        return

    if case in {"debug-inbox-protected", "debug-traversal", "read-grant-is-not-write", "write-stale-snapshot",
                "single-step-requires-pause"}:
        no_effect(d)
        for request, reply in obs:
            if case == "read-grant-is-not-write" and request["method"] == "debug.read":
                require(reply["type"] == "reply" and reply["result"]["text"] == "original",
                        "read-only authority control failed")
                continue
            error_only(reply, "conflict" if case in {"write-stale-snapshot", "single-step-requires-pause"}
                       else "permission_denied")
        return

    if case == "action-expired":
        expected = copy.deepcopy(b)
        request, reply = obs[-1]
        expected["receiver"]["rejected_actions"][request["correlation_id"]] = {
            "request": request, "accept": reply["result"],
        }
        require(stable(s) == stable(expected) and records == d["baseline_records"],
                "expired action dispatched, published outcome, executed, or mutated unrelated custody")
        require(reply["result"]["accepted"] is False
                and reply["result"]["error"]["code"] == "deadline_exceeded", "not real expiry rejection")
        return

    if case == "pause-single-step":
        require(len(records) == 2 and records[1]["step"] == 1 and records[1]["status"] == "committed"
                and bytes.fromhex(files(b)["/data/count"]) == b"1"
                and bytes.fromhex(files(s)["/data/count"]) == b"2"
                and records[1]["output_files"] == files(s) and s["managed"]["paused"] is True,
                "single-step did not run exactly once and remain paused")
        request, reply = obs[1]
        saved = operation(s, request)
        require(saved["receipt"] == reply["result"]["receipt"] and saved["receipt"]["state"] == "applied"
                and saved["receipt"]["step"] == 1, "step receipt not durable")
        terminal_audit(d, request, "applied")
        return

    if case in {"inbox-durable-dedup", "inbox-conflicting-id"}:
        require(records == d["baseline_records"], "inbox unexpectedly executed a guest")
        request = obs[0][0]
        saved = operation(s, request)
        require(saved["receipt"] == obs[0][1]["result"] and saved["receipt"]["state"] == "applied"
                and saved["receipt"]["step"] == 0, "inbox receipt not atomic")
        expected = {**request["params"]["message"], "index":0}
        raw = bytes.fromhex(files(s)["/data/inbox.jsonl"])
        require(raw == (json.dumps(expected, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode(),
                "inbox ABI line differs/duplicates/unescaped newline")
        changes = s["staged_boundary"]["changes"]
        require(len(changes) == 1 and changes[0] == {"actor":request["params"]["actor"], "ticket":None,
            "target":request["target"], "audit_id":INBOX_OP,"method":"hub.inbox","path":"/data/inbox.jsonl",
            "before":None,"after":raw.hex()}, "hub boundary origin/bytes/count wrong")
        audit = terminal_audit(d, request, "applied")
        require(audit["step"] == 0 and audit["before_hash"] != audit["after_hash"], "inbox audit not boundary-bound")
        require(set(s["receiver"]["operations"]) - set(b["receiver"]["operations"]) == {INBOX_OP}
                and list(s["receiver"]["messages"]) == [MESSAGE] and s["receiver"]["pending"] == [],
                "inbox duplicate/new-ID custody")
        wakes = [row[1] for row in s["router"]["queue"] if row[1]["kind"] == "runtime.inbox"]
        require(len(wakes) == 1 and wakes[0]["payload"] == {"index":0,"id":MESSAGE}, "inbox wake not exactly once")
        if case == "inbox-durable-dedup":
            require(obs[1][1] == obs[0][1], "retry changed original receipt")
        else:
            error_only(obs[1][1], "conflict")
        return

    if case in {"inbox-boundary-queued", "queue-backpressure"}:
        require(len(records) == 2 and records[1]["step"] == 1 and records[1]["status"] == "aborted"
                and s["receiver"]["operations"][HOLD_OP]["receipt"]["state"] == "failed",
                "held actual guest did not finish with durable attempted failure")
        if case == "inbox-boundary-queued":
            require(obs[0][1]["result"]["state"] == "queued" and obs[1][1]["result"]["state"] == "cancelled",
                    "queued/cancel observation lost")
            saved = operation(s, obs[0][0])
            require(saved["receipt"] == obs[1][1]["result"] and INBOX_OP not in s["receiver"]["pending"]
                    and "/data/inbox.jsonl" not in files(s), "cancel appended after real guest completion")
            cancel = s["receiver"]["cancellations"][obs[1][0]["correlation_id"]]
            require(cancel == {"request":obs[1][0], "receipt":saved["receipt"]}, "cancellation ID not durable")
            terminal_audit(d, obs[0][0], "cancelled")
        else:
            prepared = d["prepared"]
            require(len(prepared) == 256 and prepared[-1][1]["result"]["pending"] == 256,
                    "pressure fixture was not genuinely full")
            for request, reply in prepared[:-1]:
                saved = operation(s, request)
                require(reply["result"]["state"] == "queued", "pressure fill not queued")
                require(saved["receipt"]["state"] in {"queued", "applied", "failed"}, "admitted pressure work lost")
            queued = {id_ for id_, saved in s["receiver"]["operations"].items()
                      if saved["receipt"]["state"] == "queued"}
            require(set(s["receiver"]["pending"]) == queued
                    and len(s["receiver"]["pending"]) == len(queued), "pressure pending/receipt atomicity")
            raw = bytes.fromhex(files(s).get("/data/inbox.jsonl", ""))
            rows = [json.loads(line) for line in raw.splitlines()]
            applied = [(r, operation(s, r)) for r, _ in prepared[:-1]
                       if operation(s, r)["receipt"]["state"] == "applied"]
            require(len(rows) == len(applied), "pressure applied receipt/append atomicity")
            for index, (row, (request, saved)) in enumerate(zip(rows, applied)):
                require(row == {**request["params"]["message"], "index":index}, "pressure line provenance/index")
                terminal_audit(d, request, "applied")
            error_only(obs[0][1], "resource_exhausted")
            require(INBOX_OP not in s["receiver"]["operations"] and MESSAGE not in s["receiver"]["messages"]
                    and all(a["operation_id"] != INBOX_OP for a in s["receiver"]["audit"])
                    and all(c["audit_id"] != INBOX_OP for c in (s["staged_boundary"] or {}).get("changes", []))
                    and MESSAGE.encode() not in bytes.fromhex(files(s).get("/data/inbox.jsonl", "")),
                    "overflow consumed ID, append, boundary or audit custody")
        return
    raise AssertionError(f"missing oracle for {case}")
