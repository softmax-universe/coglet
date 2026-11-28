"""Portable gate regressions; actual Mach-O tooling is exercised by native CI."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import subprocess
import json
import hashlib

SCRIPT = Path(__file__).resolve().parents[1] / 'audit-wire-proof-release.py'


class ReleaseGateTests(unittest.TestCase):
    def test_boundary_fixture_handles_aliased_temporary_root(self):
        # macOS exposes the temporary root via /var -> /private/var. Exercise
        # that filesystem identity mismatch on every platform, without tools.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve(strict=True)
            actual = root / 'actual'
            actual.mkdir()
            alias = root / 'alias'
            alias.symlink_to(actual, target_is_directory=True)
            with patch.object(tempfile, 'tempdir', str(alias)):
                result = unittest.TestResult()
                ReleaseBoundaryTests('test_all_ten_markers_rejected_in_either_projection').run(result)
            self.assertEqual(result.testsRun, 1)
            self.assertTrue(result.wasSuccessful(), result.errors + result.failures)

    def test_release_rejects_credential_marker_in_dylib(self):
        self.assertTrue(SCRIPT.is_file(), 'missing per-image Release auditor')
        spec = importlib.util.spec_from_file_location('wire_release_audit', SCRIPT)
        assert spec is not None and spec.loader is not None
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        with self.assertRaisesRegex(ValueError, 'forbidden.*CredentialFixtureStore'):
            audit.validate_projection(b'_$sCredentialFixtureStore', b'ordinary', release=True)


    def test_scans_every_image_and_architecture(self):
        spec = importlib.util.spec_from_file_location('wire_release_audit', SCRIPT)
        assert spec is not None and spec.loader is not None
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        self.assertTrue(callable(getattr(audit, 'audit_bundle', None)), 'missing whole-bundle audit')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = root / 'Softmax.app'
            main = app / 'Contents/MacOS/Softmax'
            dylib = app / 'Contents/MacOS/Softmax.debug.dylib'
            helper = app / 'Contents/Helpers/helper'
            for image in [main, dylib, helper]:
                image.parent.mkdir(parents=True, exist_ok=True)
                image.write_bytes(b'\xca\xfe\xba\xbe' + image.name.encode())
            calls = []
            def native(args):
                calls.append(tuple(args))
                if args[1] == '-archs':
                    return b'arm64 x86_64\n'
                if args[1] == '-thin':
                    Path(args[-1]).write_bytes(b'\xcf\xfa\xed\xfe' + args[2].encode())
                    return b''
                if args[0].endswith('/nm'):
                    return b'0000000 T _ordinary\n'
                if args[0].endswith('/strings'):
                    return b'ordinary literal\n'
                self.fail(str(args))
            with patch.object(audit, 'run_tool', side_effect=native):
                rows = audit.audit_bundle(app, root / 'evidence', release=True)
            self.assertEqual(len(rows), 6)
            self.assertEqual({(r['image'], r['arch']) for r in rows},
                             {(str(p.relative_to(app)), a) for p in [main, dylib, helper] for a in ['arm64', 'x86_64']})
            self.assertEqual(sum(c[0].endswith('/nm') for c in calls), 6)
            self.assertEqual(sum(c[0].endswith('/strings') for c in calls), 6)
            for row in rows:
                image = app / row['image']
                self.assertEqual(row['image_sha256'], hashlib.sha256(image.read_bytes()).hexdigest())
                self.assertTrue((root / 'evidence' / row['symbols']).is_file())
                self.assertTrue((root / 'evidence' / row['strings']).is_file())


    def test_debug_requires_each_marker_in_each_app_architecture(self):
        spec = importlib.util.spec_from_file_location('wire_release_audit', SCRIPT)
        assert spec is not None and spec.loader is not None
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        self.assertTrue(callable(getattr(audit, 'validate_debug', None)), 'missing Debug positive control')
        rows = [{'image': 'Contents/MacOS/Softmax', 'arch': a, 'hits': []} for a in ['arm64', 'x86_64']]
        rows.append({'image': 'Contents/MacOS/Softmax.debug.dylib', 'arch': 'arm64',
                     'hits': [m.decode() for m in audit.MARKERS]})
        with self.assertRaisesRegex(ValueError, 'missing Debug.*x86_64'):
            audit.validate_debug(rows)
        rows.append({'image': 'Contents/MacOS/Softmax.debug.dylib', 'arch': 'x86_64',
                     'hits': [m.decode() for m in audit.MARKERS]})
        audit.validate_debug(rows)
        rows[-1]['hits'].remove('--sdk-wire-proof-credentials')
        with self.assertRaisesRegex(ValueError, 'missing Debug.*--sdk-wire-proof-credentials'):
            audit.validate_debug(rows)


class ReleaseBoundaryTests(unittest.TestCase):
    """Synthetic headers/tool responses test policy, not native Mach-O validity."""

    # Independent oracle: changing the implementation's marker list must not
    # silently reduce the set that these exclusion tests exercise.
    EXPECTED_MARKERS = (
        b'AppSurfaceWireProof', b'WireProofExecutor', b'wireProofShortcutPresses',
        b'WireProofCredentialFixture', b'CredentialFixtureStore', b'CredentialFixtureLink',
        b'installWireProofHubFixture', b'--sdk-wire-proof-directory',
        b'--sdk-wire-proof-credentials', b'credential-witness',
    )

    def setUp(self):
        spec = importlib.util.spec_from_file_location('wire_release_boundary_audit', SCRIPT)
        assert spec is not None and spec.loader is not None
        self.audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.audit)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve(strict=True)
        self.app = self.root / 'Softmax.app'
        self.main = self.image('Contents/MacOS/Softmax')
        self.output = self.root / 'evidence'
        self.calls = []
        self.arch_output = b'arm64 x86_64\n'
        self.projections = {}
        self.slices = {}
        self.tool_fault = None
        native = patch.object(self.audit.subprocess, 'run', side_effect=self.native)
        native.start()
        self.addCleanup(native.stop)

    def image(self, relative, *, thin=False):
        path = self.app / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes((b'\xcf\xfa\xed\xfe' if thin else b'\xca\xfe\xba\xbe') + relative.encode())
        return path

    def native(self, args, **kwargs):
        self.calls.append(tuple(args))
        self.assertEqual(kwargs, {'capture_output': True, 'check': False, 'timeout': 120})
        if args[0] == '/usr/bin/lipo' and args[1] == '-archs':
            output = self.arch_output
        elif args[0] == '/usr/bin/lipo' and args[1] == '-thin':
            path = Path(args[-1])
            path.write_bytes(b'\xcf\xfa\xed\xfe' + args[2].encode())
            self.slices[str(path)] = (str(Path(args[3]).relative_to(self.app)), args[2])
            output = b''  # Successful lipo -thin normally has no stdout.
        elif args[0] in ('/usr/bin/nm', '/usr/bin/strings'):
            image, arch = self.slices.get(args[-1],
                (str(Path(args[-1]).relative_to(self.app)) if Path(args[-1]).is_relative_to(self.app) else '', 'arm64'))
            tool = Path(args[0]).name
            output = self.projections.get((image, arch, tool), b'ordinary native inspection\n')
        else:
            self.fail(f'unexpected native invocation: {args!r}')
        if self.tool_fault and self.tool_fault[0](args):
            return subprocess.CompletedProcess(args, self.tool_fault[1], output, self.tool_fault[2])
        return subprocess.CompletedProcess(args, 0, output, b'')

    def run_audit(self, *, release=True):
        return self.audit.audit_bundle(self.app, self.output, release=release)

    def test_all_ten_markers_rejected_in_either_projection(self):
        self.assertEqual(self.audit.MARKERS, self.EXPECTED_MARKERS)
        for marker in self.EXPECTED_MARKERS:
            for channel in ('nm', 'strings'):
                with self.subTest(marker=marker, channel=channel):
                    self.projections = {('Contents/MacOS/Softmax', 'arm64', channel): b'prefix_' + marker + b'_suffix\n'}
                    with self.assertRaisesRegex(ValueError, 'forbidden wire-proof markers') as caught:
                        self.run_audit()
                    self.assertIn(marker.decode(), str(caught.exception))

    def test_fat_second_architecture_only_leakage(self):
        for channel in ('nm', 'strings'):
            with self.subTest(channel=channel):
                self.projections = {('Contents/MacOS/Softmax', 'x86_64', channel): b'CredentialFixtureLink\n'}
                with self.assertRaisesRegex(ValueError, 'forbidden.*CredentialFixtureLink'):
                    self.run_audit()
                self.assertTrue(any(c[:3] == ('/usr/bin/lipo', '-thin', 'x86_64') for c in self.calls))

    def test_dylib_only_leakage_despite_clean_primary(self):
        self.image('Contents/Frameworks/embedded.dylib')
        for channel in ('nm', 'strings'):
            with self.subTest(channel=channel):
                self.projections = {('Contents/Frameworks/embedded.dylib', 'x86_64', channel): b'credential-witness\n'}
                with self.assertRaisesRegex(ValueError, 'forbidden.*credential-witness'):
                    self.run_audit()

    def test_thin_image_is_inspected_without_lipo_thin(self):
        self.image('Contents/MacOS/Softmax', thin=True)
        self.arch_output = b'arm64\n'
        rows = self.run_audit()
        self.assertEqual([(r['image'], r['arch'], r['hits']) for r in rows],
                         [('Contents/MacOS/Softmax', 'arm64', [])])
        self.assertFalse(any(c[1] == '-thin' for c in self.calls))
        self.assertIn(('/usr/bin/nm', str(self.main)), self.calls)
        self.assertIn(('/usr/bin/strings', '-a', str(self.main)), self.calls)
        self.projections = {('Contents/MacOS/Softmax', 'arm64', 'strings'): b'WireProofExecutor'}
        with self.assertRaisesRegex(ValueError, 'forbidden.*WireProofExecutor'):
            self.run_audit()

    def test_missing_or_non_macho_primary_is_rejected_before_tools(self):
        for contents in (None, b'not a Mach-O'):
            with self.subTest(contents=contents):
                self.main.unlink(missing_ok=True)
                if contents is not None:
                    self.main.write_bytes(contents)
                with self.assertRaisesRegex(ValueError, 'missing Mach-O primary executable'):
                    self.run_audit()
                self.assertEqual(self.calls, [])

    def test_bundle_symlink_escape_is_rejected_before_tools(self):
        outside = self.root / 'outside'
        outside.write_bytes(b'\xcf\xfa\xed\xfeoutside')
        (self.app / 'Contents/escape').symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'escaping bundle symlink'):
            self.run_audit()
        self.assertEqual(self.calls, [])

    def test_each_native_tool_nonzero_or_stderr_fails_closed(self):
        stages = (
            ('archs', lambda a: a[0] == '/usr/bin/lipo' and a[1] == '-archs'),
            ('thin', lambda a: a[0] == '/usr/bin/lipo' and a[1] == '-thin'),
            ('nm', lambda a: a[0] == '/usr/bin/nm'),
            ('strings', lambda a: a[0] == '/usr/bin/strings'),
        )
        for stage, predicate in stages:
            for status, stderr in ((1, b''), (0, b'warning: incomplete inspection')):
                with self.subTest(stage=stage, status=status, stderr=stderr):
                    self.tool_fault = (predicate, status, stderr)
                    with self.assertRaisesRegex(ValueError, 'native tool failed'):
                        self.run_audit()

    def test_exact_nm_no_symbols_diagnostic_still_scans_strings(self):
        preview = self.image('Contents/MacOS/__preview.dylib', thin=True)
        self.arch_output = b'arm64\n'
        self.projections = {('Contents/MacOS/__preview.dylib', 'arm64', 'nm'): b''}
        self.tool_fault = (lambda a: a == ['/usr/bin/nm', str(preview)], 0,
                           f'{preview}: no symbols\n'.encode())
        rows = self.run_audit()
        row = next(r for r in rows if r['image'].endswith('__preview.dylib'))
        self.assertEqual((self.output / row['symbols']).read_bytes(), b'')
        self.assertEqual(row['symbols_sha256'], hashlib.sha256(b'').hexdigest())
        self.assertTrue(row['no_symbols'])
        self.assertIn(('/usr/bin/strings', '-a', str(preview)), self.calls)
        self.projections[('Contents/MacOS/__preview.dylib', 'arm64', 'strings')] = b'CredentialFixtureStore'
        with self.assertRaisesRegex(ValueError, 'forbidden.*CredentialFixtureStore'):
            self.run_audit()
        rows = self.run_audit(release=False)
        with self.assertRaisesRegex(ValueError, 'missing Debug markers'):
            self.audit.validate_debug(rows)

    def test_no_symbols_diagnostic_is_exact_and_nm_only(self):
        path = str(self.main)
        exact = f'{path}: no symbols\n'.encode()
        cases = (
            ('nonzero', ['/usr/bin/nm', path], 1, b'', exact),
            ('symbols present', ['/usr/bin/nm', path], 0, b'_ordinary\n', exact),
            ('whitespace stdout', ['/usr/bin/nm', path], 0, b'\n', exact),
            ('wrong image', ['/usr/bin/nm', path], 0, b'', b'/other: no symbols\n'),
            ('extra warning', ['/usr/bin/nm', path], 0, b'', exact + b'warning: truncated\n'),
            ('prefixed error', ['/usr/bin/nm', path], 0, b'', b'error: ' + exact),
            ('duplicate diagnostic', ['/usr/bin/nm', path], 0, b'', exact + exact),
            ('missing newline', ['/usr/bin/nm', path], 0, b'', exact[:-1]),
            ('strings', ['/usr/bin/strings', '-a', path], 0, b'', exact),
            ('lipo', ['/usr/bin/lipo', '-archs', path], 0, b'', exact),
            ('other nm', ['/other/nm', path], 0, b'', exact),
            ('extra nm argument', ['/usr/bin/nm', '-a', path], 0, b'', exact),
        )
        for name, args, status, stdout, stderr in cases:
            with self.subTest(case=name), patch.object(self.audit.subprocess, 'run',
                    return_value=subprocess.CompletedProcess(args, status, stdout, stderr)):
                with self.assertRaisesRegex(ValueError, 'native tool failed'):
                    self.audit.run_tool(args)

    def test_fat_no_symbols_checks_each_slice_and_retains_debug_positive(self):
        preview = 'Contents/MacOS/__preview.dylib'
        self.image(preview)
        all_markers = b'\n'.join(self.EXPECTED_MARKERS)
        self.projections = {('Contents/MacOS/Softmax', arch, 'nm'): all_markers
                            for arch in ('arm64', 'x86_64')}

        def native(args, **kwargs):
            result = self.native(args, **kwargs)
            if args[0] == '/usr/bin/nm' and self.slices.get(args[-1], ('', ''))[0] == preview:
                return subprocess.CompletedProcess(args, 0, b'', f'{args[-1]}: no symbols\n'.encode())
            return result

        with patch.object(self.audit.subprocess, 'run', side_effect=native):
            rows = self.run_audit(release=False)
            self.audit.validate_debug(rows)
            preview_rows = [r for r in rows if r['image'] == preview]
            self.assertEqual({r['arch'] for r in preview_rows}, {'arm64', 'x86_64'})
            self.assertTrue(all(r['no_symbols'] for r in preview_rows))
            self.projections = {(preview, 'x86_64', 'strings'): b'credential-witness'}
            with self.assertRaisesRegex(ValueError, 'forbidden.*credential-witness'):
                self.run_audit()
            self.projections = {(preview, 'arm64', 'strings'): b''}
            with self.assertRaisesRegex(ValueError, 'empty native inspection output'):
                self.run_audit()

    def test_malformed_duplicate_and_empty_architectures_rejected(self):
        for archs in (b'', b' \n', b'arm64 arm64', b'arm64 ../x86_64', b'arm64 x86-64', b'arm64\xff'):
            with self.subTest(archs=archs):
                self.arch_output = archs
                self.calls.clear()
                with self.assertRaises((ValueError, UnicodeDecodeError)):
                    self.run_audit()
                self.assertEqual(len(self.calls), 1)

    def test_thin_image_cannot_claim_multiple_architectures(self):
        self.image('Contents/MacOS/Softmax', thin=True)
        with self.assertRaisesRegex(ValueError, 'thin image has multiple architectures'):
            self.run_audit()
        self.assertEqual(len(self.calls), 1)

    def test_empty_symbol_or_string_output_is_rejected(self):
        for tool in ('nm', 'strings'):
            for output in (b'', b' \t\n'):
                with self.subTest(tool=tool, output=output):
                    self.projections = {('Contents/MacOS/Softmax', 'arm64', tool): output}
                    with self.assertRaisesRegex(ValueError, 'empty native inspection output'):
                        self.run_audit()

    def test_debug_absent_positive_control_is_rejected(self):
        rows = self.run_audit(release=False)
        self.assertEqual(len(rows), 2)
        with self.assertRaisesRegex(ValueError, 'missing Debug markers in arm64'):
            self.audit.validate_debug(rows)
        with self.assertRaisesRegex(ValueError, 'missing Debug app architecture'):
            self.audit.validate_debug([])

    def test_stale_success_report_removed_on_debug_and_release_failure(self):
        for failure in ('debug', 'release'):
            with self.subTest(failure=failure):
                self.output.mkdir(exist_ok=True)
                report = self.output / 'report.json'
                report.write_text(json.dumps({'passed': True, 'stale': True}))
                all_markers = b'\n'.join(self.EXPECTED_MARKERS)
                self.projections = {('Contents/MacOS/Softmax', arch, 'nm'): all_markers
                                    for arch in ('arm64', 'x86_64')} if failure == 'release' else {}
                argv = ['audit-wire-proof-release.py', '--debug', str(self.app),
                        '--release', str(self.app), '--output', str(self.output)]
                expected = 'forbidden wire-proof markers' if failure == 'release' else 'missing Debug markers'
                with patch('sys.argv', argv), self.assertRaisesRegex(ValueError, expected):
                    self.audit.main()
                self.assertFalse(report.exists(), 'failed invocation retained stale success report')


if __name__ == '__main__':
    unittest.main()
