"""Custody/helper regressions, NOT receiver behavioral qualification."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from receiver_oracles import error_only, no_effect, operation
from receiver_peer_driver import identity, stop_owned, teardown


class OracleHelpers(unittest.TestCase):
    def test_negative_disclosure_is_closed(self):
        reply = {"type":"error", "correlation_id":"x", "error":{
            "code":"not_found", "message":"not_found", "retryable":False}}
        error_only(reply, "not_found")
        for key in ("result", "size", "update", "sha256"):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                error_only({**reply, key:"private"}, "not_found")

    def test_no_effect_checks_every_nonclock_field_and_journal(self):
        state = {"runtime":{"time":1, "files":{"/data/a":"00"}}, "clock_anchor":{},
                 "staged_boundary":None, "receiver":{"audit":[],"pending":[]}}
        d = {"baseline":state,"state":copy.deepcopy(state),"records":[],"baseline_records":[]}
        d["state"]["runtime"]["time"] = 2
        no_effect(d)
        for mutation in (lambda x: x["state"]["receiver"]["audit"].append({}),
                         lambda x: x["state"]["runtime"]["files"].update({"/data/a":"01"}),
                         lambda x: x["records"].append({})):
            changed = copy.deepcopy(d)
            mutation(changed)
            with self.assertRaises(AssertionError):
                no_effect(changed)

    def test_original_receipt_scope_is_exact_including_ticket(self):
        request = {"method":"debug.write", "correlation_id":"x", "params":{"actor":"hub:a"}}
        saved = {"request":request,"scope":{"actor":"hub:a","method":"debug.write"},
                 "receipt":{"operation_id":"x"}}
        state = {"receiver":{"operations":{"x":saved}}}
        operation(state, request)
        saved["scope"]["ticket"] = "other"
        with self.assertRaises(AssertionError):
            operation(state, request)


class ProcessCustody(unittest.TestCase):
    def test_only_exact_live_child_can_be_cleaned(self):
        child = subprocess.Popen([sys.executable, "-m", "http.server", "0", "--bind", "127.0.0.1"], stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
                path = Path(tmp)/"custody.json"
                observed = identity(child.pid)
                assert observed is not None
                start = observed[0]
                path.write_text(json.dumps({"pid":child.pid,"start":start+"wrong"}))
                stop_owned(path)
                self.assertIsNone(child.poll())
                path.write_text(json.dumps({"pid":child.pid,"start":start}))
                stop_owned(path)
                self.assertEqual(child.wait(timeout=3), -9)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()

    def test_oracle_failure_is_teardown_failure_and_cleans_child(self):
        child = subprocess.Popen([sys.executable, "-m", "http.server", "0", "--bind", "127.0.0.1"], stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
                root = Path(tmp)
                observed = identity(child.pid)
                assert observed is not None
                (root/"action-expired.custody.json").write_text(json.dumps({
                    "pid":child.pid,"start":observed[0]}))
                (root/"action-expired.result.json").write_text(json.dumps({"ok":False,"error":"test control"}))
                with patch.dict(os.environ, {"COGLET_FIXTURE_RECEIVER_STATE":tmp,
                                             "COGLET_CONFORMANCE_CASE":"action-expired"}):
                    with self.assertRaisesRegex(RuntimeError, "actual receiver oracle failed"):
                        teardown()
                self.assertEqual(child.wait(timeout=3), -9)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


if __name__ == "__main__":
    unittest.main()
