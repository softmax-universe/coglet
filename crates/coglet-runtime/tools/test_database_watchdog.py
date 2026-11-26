#!/usr/bin/env python3
"""ADR-0041 crate-owned tests: `database_functional_watchdog` registry binding.

Black-box: drives `conformance/run` against fake stdio peers; no cargo, no real
database, no import of harness internals (so it does not depend on the names the
shared-harness implementation picks). Fakes prove watchdog behavior ONLY, never
database semantics.

Run:  python3 crates/coglet-runtime/tools/test_database_watchdog.py
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
RUN = ROOT / "conformance/run"
sys.path.insert(0, str(ROOT / "conformance"))
from wire import encode  # noqa: E402

REQ = {"op": "ping"}
RESP = {"op": "pong"}

# Fake guest: reads one frame, sleeps DELAY, then behaves per MODE.
PEER = textwrap.dedent('''
    import os, struct, sys, time
    sys.path.insert(0, %r)
    from wire import encode
    i, o = sys.stdin.buffer, sys.stdout.buffer
    open(os.environ["LAUNCH_MARKER"], "a").write("launched\\n")
    if os.environ.get("READY"):
        sys.stderr.write(os.environ["READY"] + "\\n")
        sys.stderr.flush()
    mode, delay = os.environ.get("MODE", "ok"), float(os.environ.get("DELAY", "0"))
    hdr = i.read(4)
    n = struct.unpack(">I", hdr)[0]
    i.read(n)
    time.sleep(delay)
    good = encode({"op": "pong"})
    if mode == "ok":
        o.write(struct.pack(">I", len(good)) + good)
    elif mode == "wrong":
        bad = encode({"op": "nope"})
        o.write(struct.pack(">I", len(bad)) + bad)
    elif mode == "partial":
        o.write(struct.pack(">I", len(good)) + good[:2])
    elif mode == "oversize":
        o.write(struct.pack(">I", 0xFFFFFFFF))
    elif mode == "hang":
        time.sleep(60)
    elif mode == "partialhang":
        # Declared length is the FULL frame; only a 2-byte prefix ever arrives, then silence.
        o.write(struct.pack(">I", len(good)) + good[:2])
        o.flush()
        time.sleep(60)
    elif mode == "trickle":
        # Complete valid frame delivered in 3 chunks separated by TRICKLE seconds.
        gap = float(os.environ["TRICKLE"])
        data = struct.pack(">I", len(good)) + good
        cuts = [3, 3 + (len(data) - 3) // 2, len(data)]
        last = 0
        for c in cuts:
            o.write(data[last:c])
            o.flush()
            last = c
            if c != len(data):
                time.sleep(gap)
    elif mode == "grandchild":
        import subprocess
        code = ("import os, signal, time\\n"
                "signal.signal(signal.SIGTERM, signal.SIG_IGN)\\n"
                "open(" + repr(os.environ["GRANDCHILD_PID_FILE"]) + ", 'w').write(str(os.getpid()))\\n"
                "time.sleep(120)\\n")
        subprocess.Popen([sys.executable, "-c", code])
        time.sleep(60)
    o.flush()
    # Explicitly keep the fake alive for legacy silence controls; process exit is EOF, not silence.
    time.sleep(float(os.environ.get("LINGER", "0")))
    if mode in ("ok", "wrong", "partial", "oversize", "eof", "trickle"):
        sys.exit(0)
''' % str(ROOT / "conformance"))


def toml_value(v):
    """Minimal TOML encoder for registry values (bool/int/str/list/dict)."""
    if isinstance(v, dict):
        return "{" + ", ".join(f"{json.dumps(k)} = {toml_value(x)}" for k, x in v.items()) + "}"
    if isinstance(v, list):
        return "[" + ", ".join(toml_value(x) for x in v) + "]"
    return json.dumps(v)  # bool/int/str: JSON scalars are valid TOML


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="dbw-", dir=os.environ.get("TMPDIR"))
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.marker = self.dir / "launched"
        self.hook_marker = self.dir / "hook-ran"
        (self.dir / "hook.py").write_text(
            "import os, sys\nopen(os.environ['HOOK_MARKER'], 'a').write(sys.argv[1] + '\\n')\n")
        (self.dir / "peer.py").write_text(PEER)

    def registry(self, **extra):
        impl = {"name": "fake-db", "suites": ["database"], "transport": "stdio",
                "command": [sys.executable, str(self.dir / "peer.py")],
                "peer_roles": ["guest"], "database_functional_watchdog": True}
        impl.update(extra)
        impl = {k: v for k, v in impl.items() if v is not None}
        lines = ["[[implementations]]"]
        for k, v in impl.items():
            lines.append(f"{k} = {toml_value(v)}")  # JSON scalars/arrays are valid TOML here
        path = self.dir / "impls.toml"
        path.write_text("\n".join(lines) + "\n")
        return path

    def hooks(self):
        """Fixture label `enrolled`: setup/teardown leave HOOK_MARKER lines if ever run."""
        hook = str(self.dir / "hook.py")
        return {"enrolled": {"setup": [sys.executable, hook, "setup"],
                             "teardown": [sys.executable, hook, "teardown"]}}

    def case(self, *steps, name="fn", **case_extra):
        case = {"name": name, "peer_role": "guest",
                "steps": list(steps) or [{"send": REQ}, {"expect": RESP, "within_ms": 500}]}
        case.update(case_extra)
        return case

    def write_cases(self, *cases):
        d = self.dir / "cases" / "database"
        d.mkdir(parents=True, exist_ok=True)
        (d / "c.json").write_text(json.dumps(list(cases)))
        return self.dir / "cases"

    def cases(self, *steps, name="fn", **case_extra):
        return self.write_cases(self.case(*steps, name=name, **case_extra))

    def run_harness(self, registry, cases, mode="ok", delay=0.0, timeout=40, **extra_env):
        junit = self.dir / "out.xml"
        env = dict(os.environ, MODE=mode, DELAY=str(delay), LAUNCH_MARKER=str(self.marker),
                   HOOK_MARKER=str(self.hook_marker))
        env.update({k: str(v) for k, v in extra_env.items()})
        started = time.monotonic()
        proc = subprocess.run([sys.executable, str(RUN), "--registry", str(registry), "--cases", str(cases),
                               "--junit", str(junit)], cwd=ROOT, env=env, capture_output=True,
                              text=True, timeout=timeout)
        elapsed = time.monotonic() - started
        failures = []
        if junit.exists():
            for tc in ET.parse(junit).getroot().iter("testcase"):
                for f in tc.findall("failure") + tc.findall("error"):
                    failures.append((tc.attrib["name"], f.attrib.get("message", "") + (f.text or "")))
        return proc, failures, elapsed

    def assert_rejected_before_launch(self, registry, cases, expect=None):
        proc, failures, _ = self.run_harness(registry, cases)
        self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertFalse(self.marker.exists(), "peer was launched despite invalid admission")
        self.assertFalse(self.hook_marker.exists(), "fixture hook ran despite invalid admission")
        # Exactly one failure: the harness-level configuration error, not a per-case runtime failure.
        self.assertEqual([n for n, _ in failures], ["configuration"], failures)
        if expect:
            self.assertIn(expect, failures[0][1])
        text = proc.stdout + proc.stderr + json.dumps(failures)
        # Must be an explicit configuration error mentioning the binding, not a runtime failure.
        self.assertIn("database_functional_watchdog", text)
        self.assertNotIn("step 0", text)
        return proc, failures


class RegistryAdmission(Base):
    """Invalid implementation entries fail configuration; no peer launched."""

    def test_valid_entry_is_admitted_and_passes(self):
        proc, failures, _ = self.run_harness(self.registry(), self.cases())
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)
        self.assertTrue(self.marker.exists())

    def test_nonboolean_values_rejected(self):
        for value in ("true", 1, 0, [True], {}, "yes"):
            with self.subTest(value=value):
                self.marker.unlink(missing_ok=True)
                self.assert_rejected_before_launch(self.registry(database_functional_watchdog=value), self.cases())

    def test_wrong_suite_rejected(self):
        self.assert_rejected_before_launch(self.registry(suites=["abi"]), self.cases())

    def test_mixed_suites_rejected(self):
        self.assert_rejected_before_launch(self.registry(suites=["database", "abi"]), self.cases())

    def test_wrong_role_rejected(self):
        self.assert_rejected_before_launch(self.registry(peer_roles=["runtime"]), self.cases())

    def test_mixed_roles_rejected(self):
        self.assert_rejected_before_launch(self.registry(peer_roles=["guest", "runtime"]), self.cases())

    def test_default_role_rejected(self):
        self.assert_rejected_before_launch(self.registry(peer_roles=None), self.cases())

    def test_non_stdio_transport_rejected(self):
        self.assert_rejected_before_launch(
            self.registry(transport="unix", socket=str(self.dir / "s.sock"), command=None), self.cases())

    def test_clock_and_scheduler_bindings_rejected(self):
        for extra in ({"fixture_clock": True}, {"fixture_clock": True, "fixture_scheduler": True}):
            with self.subTest(extra=extra):
                self.assert_rejected_before_launch(self.registry(**extra), self.cases())

    def test_explicit_false_clock_is_allowed(self):
        proc, failures, _ = self.run_harness(self.registry(fixture_clock=False), self.cases())
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)

    def test_unknown_field_rejected(self):
        proc, _, _ = self.run_harness(self.registry(database_functional_watchdog_ms=9000), self.cases())
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse(self.marker.exists())


class CasePreflight(Base):
    """Opted-in cases with protocol-time/transport-schedule assertions fail before launch."""

    def check(self, *steps, **case_extra):
        self.assert_rejected_before_launch(self.registry(), self.cases(*steps, **case_extra))

    def test_at_ms_rejected(self):
        self.check({"send": REQ, "at_ms": 10}, {"expect": RESP})

    def test_at_ms_zero_rejected(self):
        self.check({"send": REQ, "at_ms": 0}, {"expect": RESP})

    def test_silence_rejected(self):
        self.check({"send": REQ}, {"expect": RESP}, {"silence_ms": 10})

    def test_expect_close_rejected(self):
        self.check({"send": REQ}, {"expect": RESP}, {"expect_close": True})

    def test_stream_and_finish_rejected(self):
        self.check({"send": REQ, "stream": 1}, {"expect": RESP})
        self.marker.unlink(missing_ok=True)
        self.check({"send": REQ, "finish": False, "stream": 1}, {"expect": RESP})

    def test_non_opted_in_behavior_unchanged(self):
        # Same timed case without the binding must still be admitted and run.
        reg = self.registry(database_functional_watchdog=None)
        proc, failures, _ = self.run_harness(
            reg, self.cases({"send": REQ, "at_ms": 0}, {"expect": RESP, "within_ms": 500}, {"silence_ms": 10}), LINGER=0.2)
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)

    def test_false_binding_keeps_legacy_within_ms(self):
        reg = self.registry(database_functional_watchdog=False)
        proc, failures, _ = self.run_harness(reg, self.cases(), delay=0.9)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(any("TimeoutError" in m for _, m in failures), failures)


class WatchdogControls(Base):
    """Fake transports: watchdog behavior only (ADR-0041 Verification)."""

    def test_delay_between_500ms_and_5s_passes_when_opted_in(self):
        proc, failures, elapsed = self.run_harness(self.registry(), self.cases(), delay=1.2)
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)
        self.assertGreaterEqual(elapsed, 1.2)

    def test_same_delay_fails_when_not_opted_in(self):
        proc, failures, _ = self.run_harness(self.registry(database_functional_watchdog=None), self.cases(), delay=1.2)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(any("TimeoutError" in m for _, m in failures), failures)

    def test_hung_responder_fails_within_bounds(self):
        proc, failures, elapsed = self.run_harness(self.registry(), self.cases(), mode="hang")
        self.assertNotEqual(proc.returncode, 0)
        self.assertGreater(elapsed, 4.5, "frame watchdog must be 5000ms, not the 500ms within_ms")
        self.assertLess(elapsed, 14, "whole-case bound (10000ms) plus teardown")
        self.assertTrue(failures, proc.stdout)

    def test_wrong_response_fails(self):
        proc, failures, _ = self.run_harness(self.registry(), self.cases(), mode="wrong", delay=0.7)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(any("expected" in m for _, m in failures), failures)

    def test_partial_frame_then_eof_fails(self):
        proc, failures, _ = self.run_harness(self.registry(), self.cases(), mode="partial", delay=0.7)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(any("IncompleteReadError" in m or "EOF" in m for _, m in failures), failures)

    def test_oversized_frame_fails(self):
        proc, failures, _ = self.run_harness(self.registry(), self.cases(), mode="oversize", delay=0.7)
        self.assertNotIn('configuration', [n for n, _ in failures], 'config error is not a watchdog failure')
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(failures)

    def test_eof_without_response_fails(self):
        # MODE=eof: reads request, exits without answering.
        proc, failures, _ = self.run_harness(self.registry(), self.cases(), mode="eof", delay=0.7)
        self.assertNotIn('configuration', [n for n, _ in failures], 'config error is not a watchdog failure')
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(failures)

    def test_frame_watchdog_is_not_whole_case_or_startup_bound(self):
        # A 7s response is beyond the 5000ms frame watchdog: must still fail.
        proc, failures, elapsed = self.run_harness(self.registry(), self.cases(), delay=7)
        self.assertNotEqual(proc.returncode, 0)
        self.assertLess(elapsed, 14)
        self.assertTrue(any("TimeoutError" in m for _, m in failures), failures)

    def test_whole_case_bound_unchanged(self):
        proc, failures, elapsed = self.run_harness(
            self.registry(), self.cases(timeout_ms=2000), mode="hang")
        self.assertNotIn('configuration', [n for n, _ in failures], 'config error is not a watchdog failure')
        self.assertNotEqual(proc.returncode, 0)
        self.assertLess(elapsed, 6, "case timeout_ms must still cap the case")

    def test_process_group_reaped_after_failure(self):
        proc, failures, _ = self.run_harness(self.registry(), self.cases(timeout_ms=1500), mode="hang")
        self.assertNotIn('configuration', [n for n, _ in failures], 'config error is not a watchdog failure')
        self.assertNotEqual(proc.returncode, 0)
        out = subprocess.run(["pgrep", "-f", str(self.dir / "peer.py")], capture_output=True, text=True)
        self.assertEqual(out.stdout.strip(), "", "fake peer left running")


FORBIDDEN_STEPS = {
    "at_ms": [{"send": REQ, "at_ms": 10}, {"expect": RESP}],
    "silence_ms": [{"send": REQ}, {"expect": RESP}, {"silence_ms": 10}],
    "expect_close": [{"send": REQ}, {"expect": RESP}, {"expect_close": True}],
    "stream": [{"send": REQ, "stream": 1}, {"expect": RESP}],
    "finish": [{"send": REQ, "stream": 1, "finish": False}, {"expect": RESP}],
}


class GlobalPreflight(Base):
    """An invalid opted-in case ANYWHERE in the run launches no hook and no peer,
    including when an earlier valid case (which would otherwise run first) precedes it."""

    def hooked_registry(self, **extra):
        return self.registry(fixtures=self.hooks(), **extra)

    def good(self, name="aa_valid"):
        return self.case(name=name, fixtures=["enrolled"])

    def bad(self, control, name="zz_invalid"):
        return self.case(*FORBIDDEN_STEPS[control], name=name, fixtures=["enrolled"])

    def test_invalid_later_case_blocks_valid_earlier_case(self):
        for control in FORBIDDEN_STEPS:
            with self.subTest(control=control):
                self.marker.unlink(missing_ok=True)
                cases = self.write_cases(self.good(), self.bad(control))
                _, failures = self.assert_rejected_before_launch(self.hooked_registry(), cases, expect="zz_invalid")
                self.assertIn(control, failures[0][1])
                self.assertNotIn("aa_valid", failures[0][1])

    def test_invalid_earlier_case_blocks_valid_later_case(self):
        for control in FORBIDDEN_STEPS:
            with self.subTest(control=control):
                cases = self.write_cases(self.bad(control, name="aa_invalid"), self.good("zz_valid"))
                self.assert_rejected_before_launch(self.hooked_registry(), cases, expect="aa_invalid")

    def test_invalid_case_in_second_file_blocks_valid_first_file(self):
        cases = self.write_cases(self.good())
        (cases / "database" / "z.json").write_text(json.dumps([self.bad("at_ms")]))
        self.assert_rejected_before_launch(self.hooked_registry(), cases, expect="zz_invalid")

    def test_non_guest_case_role_rejected_not_skipped(self):
        # Without preflight a role the peer does not support would just be SKIPped.
        cases = self.write_cases(self.good(), self.case(name="zz_role", peer_role="runtime"))
        self.assert_rejected_before_launch(self.hooked_registry(), cases, expect="zz_role")

    def test_invalid_opted_in_impl_blocks_valid_earlier_nonopted_impl(self):
        py = sys.executable
        common = {"suites": ["database"], "transport": "stdio", "peer_roles": ["guest"],
                  "command": [py, str(self.dir / "peer.py")], "fixtures": self.hooks()}
        impls = [dict(common, name="a-plain"), dict(common, name="b-optin", database_functional_watchdog=True)]
        lines = []
        for impl in impls:
            lines.append("[[implementations]]")
            lines += [f"{k} = {toml_value(v)}" for k, v in impl.items()]
        reg = self.dir / "two.toml"
        reg.write_text("\n".join(lines) + "\n")
        # at_ms is legal for the plain implementation, illegal for the opted-in one.
        cases = self.write_cases(self.bad("at_ms", name="timed"))
        self.assert_rejected_before_launch(reg, cases, expect="b-optin.database/timed")

    # Controls: the same cases are runnable when the preflight does not apply.
    def test_control_valid_opted_in_runs_hooks_and_peer(self):
        proc, failures, _ = self.run_harness(self.hooked_registry(), self.write_cases(self.good()))
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)
        self.assertEqual(self.marker.read_text().splitlines(), ["launched"])
        self.assertEqual(self.hook_marker.read_text().splitlines(), ["setup", "teardown"])

    def test_control_same_invalid_cases_run_when_not_opted_in(self):
        for binding in (None, False):
            with self.subTest(binding=binding):
                self.marker.unlink(missing_ok=True)
                self.hook_marker.unlink(missing_ok=True)
                reg = self.hooked_registry(database_functional_watchdog=binding)
                proc, failures, _ = self.run_harness(
                    reg, self.write_cases(self.good(), self.bad("at_ms"), self.bad("expect_close", name="zz_close")))
                self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)
                self.assertEqual(self.marker.read_text().splitlines(), ["launched"] * 3)
                self.assertEqual(self.hook_marker.read_text().splitlines(), ["setup", "teardown"] * 3)

    def test_unknown_case_field_rejected_before_launch(self):
        cases = self.write_cases(self.good(), self.case(name="zz_unknown", fixtures=["enrolled"],
                                                         database_functional_watchdog=True))
        proc, failures, _ = self.run_harness(self.hooked_registry(), cases)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([n for n, _ in failures], ["configuration"], failures)
        self.assertFalse(self.marker.exists())
        self.assertFalse(self.hook_marker.exists())


class RegistryMalformedAndFalse(Base):
    def test_false_does_not_inherit_admission_restrictions(self):
        # `false` is the legacy path: mixed roles and a fixture clock claim are not this binding's business.
        for extra in ({"peer_roles": ["guest", "runtime"]}, {"fixture_clock": False}):
            with self.subTest(extra=extra):
                reg = self.registry(database_functional_watchdog=False, **extra)
                proc, failures, _ = self.run_harness(reg, self.cases())
                self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)

    def test_true_with_conflicts_names_the_binding_and_reason(self):
        for extra, why in (({"suites": ["database", "abi"]}, "suites"),
                           ({"peer_roles": ["guest", "runtime"]}, "peer_roles"),
                           ({"fixture_clock": True}, "fixture_clock")):
            with self.subTest(extra=extra):
                self.assert_rejected_before_launch(self.registry(**extra), self.cases(), expect=why)

    def test_nonboolean_message_says_boolean(self):
        for value in ("true", 1, [True], {}):
            with self.subTest(value=value):
                self.assert_rejected_before_launch(
                    self.registry(database_functional_watchdog=value), self.cases(), expect="boolean")


class StartupBound(Base):
    """Startup watchdog is separate from (and not extended by) the 5000ms frame watchdog."""

    READY = "COGLET_CONFORMANCE_READY"

    def run_never_ready(self, **registry_extra):
        reg = self.registry(ready_line=self.READY, **registry_extra)
        # Case budget far above startup so only the startup bound can fire.
        return self.run_harness(reg, self.cases(timeout_ms=30000))

    def test_explicit_connect_timeout_bounds_never_ready_peer(self):
        proc, failures, elapsed = self.run_never_ready(connect_timeout_ms=700)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([n for n, _ in failures], ["fn"], failures)
        message = failures[0][1]
        self.assertTrue(message.startswith("TimeoutError"), message)
        self.assertNotIn("step ", message, "must fail in startup, before any step ran")
        self.assertTrue(self.marker.exists(), "peer should have been launched (then never became ready)")
        self.assertGreaterEqual(elapsed, 0.6)
        self.assertLess(elapsed, 4.0, "700ms startup bound must not be stretched to the 5000ms frame watchdog")

    def test_default_startup_bound_is_5000ms(self):
        proc, failures, elapsed = self.run_never_ready()
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([n for n, _ in failures], ["fn"], failures)
        self.assertTrue(failures[0][1].startswith("TimeoutError"), failures[0][1])
        self.assertNotIn("step ", failures[0][1])
        self.assertGreaterEqual(elapsed, 4.8)   # lower edge is the 5s bound itself, not process spawn time
        self.assertLess(elapsed, 12.0)          # well under the 30s case budget

    def test_opted_in_and_legacy_startup_bounds_agree(self):
        for binding in (True, None):
            with self.subTest(binding=binding):
                proc, failures, elapsed = self.run_never_ready(
                    connect_timeout_ms=700, database_functional_watchdog=binding)
                self.assertTrue(failures and failures[0][1].startswith("TimeoutError"), failures)
                self.assertLess(elapsed, 4.0)

    def test_control_ready_peer_starts_within_bound(self):
        reg = self.registry(ready_line=self.READY, connect_timeout_ms=3000)
        proc, failures, _ = self.run_harness(reg, self.cases(), READY=self.READY)
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)

    def test_slow_response_after_ready_not_charged_to_startup(self):
        reg = self.registry(ready_line=self.READY, connect_timeout_ms=700)
        proc, failures, _ = self.run_harness(reg, self.cases(), READY=self.READY, delay=1.5)
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)


class FullFrameWait(Base):
    """The watchdog covers the COMPLETE declared frame, not just its length prefix."""

    def test_declared_length_then_few_bytes_then_hang_times_out_mid_frame(self):
        proc, failures, elapsed = self.run_harness(self.registry(), self.cases(timeout_ms=30000), mode="partialhang")
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([n for n, _ in failures], ["fn"], failures)
        message = failures[0][1]
        self.assertTrue(message.startswith("AssertionError: step 1: TimeoutError"), message)
        self.assertNotIn("IncompleteReadError", message, "peer never closed; this is a hang, not EOF")
        self.assertGreaterEqual(elapsed, 4.8)   # waited the whole 5000ms for the rest of the frame
        self.assertLess(elapsed, 12.0)          # ...but not the 30s case budget or the peer's 60s sleep

    def test_frame_trickled_over_multiple_reads_within_watchdog_passes(self):
        # 2 gaps x 1.2s = 2.4s of mid-frame silence in total, > 500ms within_ms, < 5000ms.
        proc, failures, elapsed = self.run_harness(self.registry(), self.cases(), mode="trickle", TRICKLE=1.2)
        self.assertEqual((proc.returncode, failures), (0, []), proc.stdout + proc.stderr)
        self.assertGreaterEqual(elapsed, 2.4)

    def test_frame_trickle_total_beyond_watchdog_fails_even_if_each_gap_is_short(self):
        # Each gap 3s (<5s) but the full frame takes 6s: the bound is per COMPLETE frame.
        proc, failures, _ = self.run_harness(self.registry(), self.cases(timeout_ms=30000),
                                             mode="trickle", TRICKLE=3.0)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(failures and failures[0][1].startswith("AssertionError: step 1: TimeoutError"), failures)

    def test_control_same_trickle_fails_without_binding(self):
        proc, failures, _ = self.run_harness(self.registry(database_functional_watchdog=None), self.cases(),
                                             mode="trickle", TRICKLE=1.2)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(failures and failures[0][1].startswith("AssertionError: step 1: TimeoutError"), failures)


class DescendantTeardown(Base):
    """A real descendant (not just the launched parent) must be gone after failure."""

    @staticmethod
    def alive(pid):
        status = subprocess.run(["ps", "-p", str(pid), "-o", "stat="],
                                capture_output=True, text=True, check=False)
        if status.returncode not in (0, 1):
            raise AssertionError(f"cannot inspect descendant {pid}: {status.stderr}")
        state = status.stdout.strip()
        return bool(state) and not state.startswith("Z")

    def test_sigterm_ignoring_grandchild_is_killed_after_case_timeout(self):
        pid_file = self.dir / "grandchild.pid"
        proc, failures, elapsed = self.run_harness(
            self.registry(), self.cases(timeout_ms=1500), mode="grandchild", GRANDCHILD_PID_FILE=pid_file)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([n for n, _ in failures], ["fn"], failures)
        self.assertTrue(failures[0][1].startswith("TimeoutError"), failures[0][1])
        self.assertTrue(pid_file.exists(), "grandchild never started; test would prove nothing")
        pid = int(pid_file.read_text())
        self.assertNotEqual(pid, os.getpid())
        deadline = time.monotonic() + 5
        while self.alive(pid) and time.monotonic() < deadline:
            time.sleep(0.05)
        still = self.alive(pid)
        if still:
            os.kill(pid, 9)  # do not leak from a failing test
        self.assertFalse(still, f"grandchild {pid} survived harness teardown")
        self.assertLess(elapsed, 8)

    def test_grandchild_killed_after_frame_watchdog_failure(self):
        pid_file = self.dir / "grandchild.pid"
        proc, failures, _ = self.run_harness(
            self.registry(), self.cases(timeout_ms=30000), mode="grandchild", GRANDCHILD_PID_FILE=pid_file)
        self.assertNotEqual(proc.returncode, 0)
        self.assertTrue(failures[0][1].startswith("AssertionError: step 1: TimeoutError"), failures)
        pid = int(pid_file.read_text())
        deadline = time.monotonic() + 5
        while self.alive(pid) and time.monotonic() < deadline:
            time.sleep(0.05)
        still = self.alive(pid)
        if still:
            os.kill(pid, 9)
        self.assertFalse(still, f"grandchild {pid} survived harness teardown")


class FrozenCorpusUnchanged(unittest.TestCase):
    def test_database_cases_have_no_time_controls(self):
        cases = json.loads((ROOT / "conformance/database/cases.json").read_text())
        for case in cases:
            for step in case["steps"]:
                self.assertFalse({"at_ms", "silence_ms", "expect_close", "stream", "finish"} & set(step),
                                 case["name"])
        self.assertTrue(all(c.get("peer_role") == "guest" for c in cases))


if __name__ == "__main__":
    unittest.main()
