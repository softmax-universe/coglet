#!/usr/bin/env python3
"""Parser controls only; synthetic records NEVER count as native execution."""
import contextlib
import fnmatch
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest import mock

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

class DiagnosticWrapperTests(unittest.TestCase):
    def run_wrapper(self,exc,secondary=False):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d)/"r3-runtime-x86_64";out.mkdir()
            seal=mock.Mock()
            patches=[mock.patch.object(check,"run_inventory",side_effect=exc),mock.patch.object(check,"seal",seal)]
            if secondary:patches.append(mock.patch.object(check,"_create_new",side_effect=OSError("disk full")))
            with contextlib.ExitStack() as stack,contextlib.redirect_stderr(io.StringIO()) as err:
                for p in patches:stack.enter_context(p)
                with self.assertRaises(type(exc)) as caught:check.run_and_seal_runtime(out)
            self.assertIs(caught.exception,exc)
            seal.assert_not_called()
            diag=check.diagnostics_dir(out)
            self.assertEqual(diag,Path(d)/"r3-runtime-x86_64-diagnostics")
            self.assertFalse((out/"manifest.json").exists())
            files=sorted(p.name for p in diag.iterdir()) if diag.exists() else []
            return files,err.getvalue(),(json.loads((diag/"runner-exception.json").read_text()) if files else None)
    def test_subprocess_failure_is_diagnosed_and_reraised_unsealed(self):
        exc=subprocess.CalledProcessError(101,["r3_runtime"])
        files,_,doc=self.run_wrapper(exc)
        self.assertEqual(files,["runner-exception.json"]);self.assertEqual(doc["type"],"CalledProcessError")
    def test_validation_assertion_is_diagnosed_and_reraised_unsealed(self):
        files,_,doc=self.run_wrapper(AssertionError("bad report"))
        self.assertEqual(files,["runner-exception.json"]);self.assertEqual(doc["message"],"bad report")
    def test_secondary_write_failure_reraises_original_and_reports_on_stderr(self):
        exc=subprocess.CalledProcessError(101,["r3_runtime"])
        files,err,_=self.run_wrapper(exc,secondary=True)
        self.assertEqual(files,[]);self.assertIn("secondary failure",err);self.assertIn("disk full",err)
    def test_existing_diagnostic_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d)/"r3-runtime-x86_64";diag=check.diagnostics_dir(out);diag.mkdir()
            (diag/"runner-exception.json").write_bytes(b"prior")
            check.write_failure_diagnostic(out,RuntimeError("x"))
            self.assertEqual((diag/"runner-exception.json").read_bytes(),b"prior")
            self.assertTrue((diag/"runner-exception-1.json").exists())
    def test_success_path_seals_once(self):
        with tempfile.TemporaryDirectory() as d,mock.patch.object(check,"run_inventory"),mock.patch.object(check,"seal") as seal:
            out=Path(d)/"r3-runtime-x86_64";check.run_and_seal_runtime(out)
            seal.assert_called_once_with(out,check.RUNTIME_FILES);self.assertFalse(check.diagnostics_dir(out).exists())

class WorkflowDiagnosticTests(unittest.TestCase):
    text=(Path(__file__).resolve().parents[2]/".github/workflows/rust.yml").read_text()
    def steps(self):
        return [b for b in re.split(r"\n      - ",self.text) if "r3-diagnostics-" in b]
    def test_diagnostic_uploads_are_separate_always_and_correct(self):
        steps=self.steps();self.assertEqual(len(steps),2)
        want={"x86_64":"r3-runtime-x86_64-diagnostics","aarch64":"r3-runtime-aarch64-diagnostics"}
        for arch,path in want.items():
            [s]=[s for s in steps if f"name: r3-diagnostics-{arch}-${{{{ inputs.bundle-mode || 'normal' }}}}" in s]
            self.assertIn("if: always() && needs.source.outputs.r3-required == 'true'",s)
            self.assertIn(f"path: ${{{{ runner.temp }}}}/{path}\n",s+"\n")
            self.assertIn("if-no-files-found: warn",s);self.assertNotIn("if-no-files-found: error",s)
    def test_names_are_excluded_by_existing_aggregator_wildcard(self):
        [pattern]=re.findall(r"pattern: (r3-runtime-\*-.*)",self.text)
        glob=pattern.replace("${{ inputs.bundle-mode || 'normal' }}","normal")
        self.assertEqual(glob,"r3-runtime-*-normal")
        for arch in ("x86_64","aarch64"):
            self.assertFalse(fnmatch.fnmatchcase(f"r3-diagnostics-{arch}-normal",glob))
            self.assertTrue(fnmatch.fnmatchcase(f"r3-runtime-{arch}-normal",glob))
    def test_existing_success_uploads_unchanged_and_required(self):
        for name in ("r3-publication-","r3-runtime-x86_64-","r3-runtime-aarch64-"):
            [s]=[s for s in re.split(r"\n      - ",self.text) if f"name: {name}${{{{" in s and "upload-artifact" in s]
            self.assertIn("if: needs.source.outputs.r3-required == 'true'",s);self.assertIn("if-no-files-found: error",s)
    def test_missing_diagnostics_are_not_success_proof(self):
        # Diagnostics are only ever uploaded (warn); nothing downloads or requires them.
        self.assertEqual(self.text.count("r3-diagnostics-"),2)
        self.assertNotIn("r3-diagnostics-*",self.text)
        self.assertNotIn("download-artifact",self.text.split("r3-diagnostics-")[1].split("\n")[0])
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);PairTests("test_accepts_complete_identical_executed_inventory").pair(p)
            (p/"r3-runtime-x86_64-diagnostics").mkdir()
            with self.assertRaises(AssertionError):check.verify_runtime_pair(p,"a"*40,"123","1")

if __name__=="__main__":unittest.main()
