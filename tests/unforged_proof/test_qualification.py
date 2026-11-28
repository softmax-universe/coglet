"""LOCAL orchestration regressions, never hosted execution evidence."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import qualification as q

class LocalQualificationTests(unittest.TestCase):
    def test_exact_seven_opt_in_names_not_unittest_discovery(self):
        expected={'hosted_replay_matches_recorded_step_trees','hosted_fork_matches_recorded_step_trees',
            'hosted_factory_gate_requires_fresh_execution','hosted_changed_image_invalidates_acceptance',
            'hosted_missing_receipt_fails_closed','hosted_private_export_requires_grant','hosted_replay_has_no_live_effects'}
        self.assertEqual(set(q.NAMES),expected)
        self.assertEqual({n for n in vars(q.Qualification) if n.startswith('hosted_')},expected)
        self.assertFalse(issubclass(q.Qualification,unittest.TestCase))
    def test_missing_config_and_partial_pins_fail_before_output(self):
        with tempfile.TemporaryDirectory() as td:
            p=Path(td)/'out'
            for config in ({},{'source':'a'*40},{'source':'wrong','driver':'/nonexistent'}):
                with self.assertRaises(q.Unavailable):q.Qualification(config,p)
                self.assertFalse(p.exists())
    def test_wrong_binary_and_source_fail_before_output(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);binary=root/'binary';binary.write_bytes(b'fake')
            config={k:str(binary) for k in ('driver','bridge','daemon','proof')}
            config.update({k+'_sha256':'0'*64 for k in ('driver','bridge','daemon','proof')})
            config.update(source='a'*40,endpoint='b'*64,image='native@sha256:'+'c'*64)
            with self.assertRaises(q.Unavailable):q.Qualification(config,root/'out')
            self.assertFalse((root/'out').exists())
    def test_fresh_output_rejects_existing_directory_and_symlink(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);target=root/'used';target.mkdir();link=root/'link';link.symlink_to(target)
            for path in (target,link):
                with self.assertRaises(q.Unavailable):q.fresh_output(path)
    def test_command_failure_retained_and_secrets_redacted(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)
            with patch.dict(os.environ,{'CAOS_IROH_TICKET':'LOCAL-SECRET'}):
                result=q.command([sys.executable,'-c','import sys;print("LOCAL-SECRET");sys.exit(7)'],root/'command',local=True)
            self.assertEqual(result.returncode,7)
            self.assertNotIn(b'LOCAL-SECRET',(root/'command.stdout').read_bytes())
            self.assertEqual(json.loads((root/'command.json').read_bytes())['returncode'],7)
            with self.assertRaises(q.Unavailable):q.require_success(result)
    def test_timeout_is_retained_not_a_negative_pass(self):
        with tempfile.TemporaryDirectory() as td:
            with patch('qualification.subprocess.run',side_effect=subprocess.TimeoutExpired(['local'],1,output=b'partial')):
                with self.assertRaises(q.Unavailable):q.command(['local'],Path(td)/'cmd',local=True)
            self.assertTrue((Path(td)/'cmd.json').exists())
            self.assertEqual((Path(td)/'cmd.stdout').read_bytes(),b'partial')
    def test_expected_refusal_does_not_accept_unrelated_crash(self):
        for code,stderr in [(0,b'known'),(1,b'wrong'),(-9,b'known')]:
            with self.assertRaises(q.Unavailable):q.require_refusal(subprocess.CompletedProcess([],code,b'',stderr),'known')
        q.require_refusal(subprocess.CompletedProcess([],1,b'',b'Refused: known'),'Refused: known')
    def test_changed_image_and_source_audit_cannot_be_waived(self):
        instance=object.__new__(q.Qualification)
        with patch.object(q.Qualification,'model',return_value=None),patch.object(q.Qualification,'receipt_negatives',return_value={'LOCAL':True}):
            with self.assertRaisesRegex(q.Unavailable,'actual.*image'):instance.hosted_changed_image_invalidates_acceptance()
    def test_empty_or_non_effectful_oracle_refuses(self):
        for trace in ({'records':[]},{'records':[{'llm_requests':[],'actions':[]}]}):
            with self.assertRaises(q.Unavailable):q.require_effects(trace,'llm_requests')
        q.require_effects({'records':[{'llm_requests':[{}]}]},'llm_requests')

if __name__=='__main__':unittest.main()
