#!/usr/bin/env python3
"""Parser controls only; synthetic records NEVER count as native execution."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location("r3_check",Path(__file__).with_name("check_r3.py"))
assert spec is not None and spec.loader is not None
check=importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

class PairTests(unittest.TestCase):
    def pair(self, root):
        for arch in ("x86_64","aarch64"):
            p=root/arch
            p.mkdir()
            runtime={"arch":arch,"mode":"native","cases":3,"wasm_sha256":"a"*64,"setup_sha256":"b"*64,"publication_sha256":"c"*64,"initial_hash":"d"*64,"state_hashes":["1"*64,"2"*64,"2"*64],"replay_hashes":["1"*64,"2"*64,"2"*64],"statuses":["committed","committed","aborted"]}
            (p/"runtime.json").write_text(json.dumps(runtime))
            records=[{"status":s,"output_hash":h,"engine":check.APPROVED_ENGINE,"calls":[{"op":"log","result":0,"argument":{"message":f"parser-control-{n}"}}],"seed":n,"logical_time":10+n,"events":[],"input_hash":"d"*64,"code_hash":"a"*64,"manifest_hash":"b"*64} for n,(s,h) in enumerate(zip(runtime["statuses"],runtime["state_hashes"]))]
            records[2]["failure"]={"kind":"trap"}
            (p/"records.json").write_text(json.dumps(records))
            for name in ("r3_runtime","r3_runtime.build.jsonl","rustc.txt","Cargo.lock"):
                (p/name).write_text("synthetic parser control only")
            (p/"runtime.log").write_text("running 1 test\ntest published_r3_runtime_journal_fs_trap_payload_chain ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n")
            self.seal(p,arch)
    def seal(self,p,arch):
        files={f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in p.iterdir() if f.name!="manifest.json"}
        (p/"manifest.json").write_text(json.dumps({"head":"a"*40,"run_id":"123","run_attempt":"1","arch":arch,"mode":"native","files":files}))
    def verify(self,root):
        return check.verify_runtime_pair(root,"a"*40,"123","1")
    def test_accepts_complete_identical_executed_inventory(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);self.pair(p);self.verify(p)
    def test_rejects_missing_leg(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);self.pair(p)
            import shutil
            shutil.rmtree(p/"aarch64")
            with self.assertRaises(AssertionError):self.verify(p)
    def test_rejects_mismatched_raw_state_hash(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);self.pair(p);a=p/"aarch64"
            r=json.loads((a/"runtime.json").read_text());r["state_hashes"][0]="e"*64;r["replay_hashes"]=r["state_hashes"]
            (a/"runtime.json").write_text(json.dumps(r));self.seal(a,"aarch64")
            with self.assertRaises(AssertionError):self.verify(p)
    def test_rejects_wrong_run_head_arch_mode_and_missing_records(self):
        for field,value in [("head","f"*40),("run_id","999"),("run_attempt","2"),("arch","x86_64"),("mode","emulated")]:
            with self.subTest(field=field),tempfile.TemporaryDirectory() as d:
                p=Path(d);self.pair(p);a=p/"aarch64"
                m=json.loads((a/"manifest.json").read_text());m[field]=value;(a/"manifest.json").write_text(json.dumps(m))
                with self.assertRaises(AssertionError):self.verify(p)
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);self.pair(p);(p/"aarch64"/"records.json").unlink()
            with self.assertRaises(AssertionError):self.verify(p)
    def test_rejects_zero_skipped_filtered_or_renamed_test(self):
        for text in ["test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out", "test published_r3_runtime_journal_fs_trap_payload_chain ... ignored", "test different ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"]:
            with self.subTest(text=text),tempfile.TemporaryDirectory() as d:
                p=Path(d);self.pair(p);a=p/"aarch64";(a/"runtime.log").write_text(text);self.seal(a,"aarch64")
                with self.assertRaises(AssertionError):self.verify(p)
    def test_rejects_divergent_raw_calls_and_unqualified_engine(self):
        for field, value in [("calls", [{"op":"log","result":0,"argument":{"message":"changed raw bits"}}]), ("engine", {"build":"unqualified","policy":"unqualified"})]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as d:
                p=Path(d);self.pair(p);a=p/"aarch64"
                records=json.loads((a/"records.json").read_text())
                records[2][field]=value
                (a/"records.json").write_text(json.dumps(records));self.seal(a,"aarch64")
                with self.assertRaises(AssertionError):self.verify(p)
    def test_publication_requires_every_indexed_evidence_blob(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);blob=b"public parser-only evidence";pin=hashlib.sha256(blob).hexdigest()
            (p/"evidence").mkdir()
            (p/"publication.json").write_text(json.dumps({"reproduction":{"evidence":{"blobs":{"raw-ci":pin}}}}))
            with self.assertRaises(FileNotFoundError):check.publication_files(p)
            (p/"evidence"/pin).write_bytes(blob)
            self.assertIn(f"evidence/{pin}",check.publication_files(p))
            (p/"evidence"/pin).write_bytes(b"changed")
            with self.assertRaises(AssertionError):check.publication_files(p)
    def test_rejects_changed_retained_bytes(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);self.pair(p);(p/"aarch64"/"runtime.json").write_text("{}")
            with self.assertRaises(AssertionError):self.verify(p)

if __name__=="__main__":unittest.main()
