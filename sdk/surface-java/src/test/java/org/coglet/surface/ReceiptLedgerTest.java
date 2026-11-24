package org.coglet.surface;

import static org.junit.jupiter.api.Assertions.*;
import java.math.BigInteger;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.*;

@Timeout(10)
class ReceiptLedgerTest {
    static final String SURFACE = "surface:" + "a".repeat(64);
    static final String COGLET = "coglet:" + "b".repeat(64);
    Path parent;
    Path ledger;
    @BeforeEach void setup() throws Exception {
        parent = Files.createTempDirectory(Path.of(System.getenv().getOrDefault("TMPDIR", System.getProperty("java.io.tmpdir"))), "cg-ledger-");
        ledger = parent.resolve("ledger");
    }
    @AfterEach void cleanup() throws Exception {
        try (var tree = Files.walk(parent)) {
            for (Path p : tree.sorted(Comparator.reverseOrder()).toList()) Files.deleteIfExists(p);
        }
    }
    static Map<String,Object> act() {
        return Map.of("type", "act", "body", Map.of("grant_epoch", 1, "action", Map.of(
                "id", "2".repeat(32), "coglet", COGLET, "capability", "expression.set_expression",
                "args", Map.of("expression", "neutral"), "deadline", 100, "correlation_id", FrameCodecTest.ID)));
    }
    static Map<String,Object> accepted() {
        return Map.of("id", "2".repeat(32), "correlation_id", FrameCodecTest.ID, "accepted", true);
    }
    @Test void interruptedSessionLatchesKillAcrossRepeatedReopens() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.beginSession();
            store.advanceEpoch(BigInteger.valueOf(7));
            store.record(act(), accepted(), BigInteger.TEN);
            // close releases ownership, but is deliberately NOT a clean shutdown.
        }
        for (int i = 0; i < 2; i++) {
            try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
                assertTrue(store.killed(), "unclean restart must durably latch kill");
                assertEquals(BigInteger.valueOf(7), store.epoch());
                assertThrows(java.io.IOException.class, store::beginSession);
                store.recoverPending(BigInteger.valueOf(120));
                assertEquals("failed", store.lookup(COGLET, "2".repeat(32)).orElseThrow()
                        .outcome().orElseThrow().get("status"));
            }
        }
    }
    @Test void cleanShutdownRequiresTerminalReceiptsAndPreservesKill() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.beginSession();
            assertThrows(java.io.IOException.class, store::beginSession);
            store.record(act(), accepted(), BigInteger.TEN);
            assertThrows(java.io.IOException.class, store::cleanShutdown);
            store.recoverPending(BigInteger.valueOf(120));
            store.cleanShutdown();
        }
        try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
            assertFalse(store.killed());
            store.beginSession();
            store.kill();
            store.cleanShutdown();
        }
        try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
            assertTrue(store.killed());
            assertThrows(java.io.IOException.class, store::beginSession);
        }
    }
    @Test void clockHighWaterSurvivesCleanRestartAndRefusesRollback() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.beginSession();
            store.checkpointClock(BigInteger.valueOf(300));
            assertThrows(IllegalArgumentException.class, () -> store.checkpointClock(BigInteger.valueOf(299)));
            store.cleanShutdown();
        }
        try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
            assertEquals(BigInteger.valueOf(300), store.clockHighWater());
            assertThrows(IllegalArgumentException.class, () -> store.checkpointClock(BigInteger.valueOf(299)));
            store.checkpointClock(BigInteger.valueOf(300));
            store.checkpointClock(BigInteger.valueOf(301));
            assertEquals(BigInteger.valueOf(301), store.clockHighWater());
        }
    }
    @Test void timestampedLedgerWritesRejectRetainedClockRollback() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(act(), accepted(), BigInteger.TEN);
            store.checkpointClock(BigInteger.valueOf(300));
            assertThrows(IllegalArgumentException.class, () -> store.record(act(), accepted(), BigInteger.valueOf(299)));
            assertThrows(IllegalArgumentException.class, () -> store.recoverPending(BigInteger.valueOf(299)));
            assertThrows(IllegalArgumentException.class, () -> store.prune(BigInteger.valueOf(299)));
            assertThrows(IllegalArgumentException.class, () -> store.finish(COGLET, "2".repeat(32),
                    Map.of("id", "2".repeat(32), "correlation_id", FrameCodecTest.ID, "status", "succeeded", "result", Map.of()),
                    BigInteger.valueOf(299)));
            store.recoverPending(BigInteger.valueOf(301));
            assertEquals(BigInteger.valueOf(301), store.clockHighWater());
        }
    }
    @Test void lifecycleCommitsFailClosedAtEveryCrashPoint() throws Exception {
        for (var point : ReceiptLedger.CommitPoint.values()) {
            Path path = parent.resolve("start-" + point);
            var inject = new java.util.concurrent.atomic.AtomicBoolean();
            try (var store = new ReceiptLedger(path, SURFACE, true, () -> {}, phase -> {
                if (inject.get() && phase == point) throw new java.io.IOException("injected");
            })) {
                inject.set(true);
                assertThrows(java.io.IOException.class, store::beginSession);
                assertThrows(java.io.IOException.class, store::beginSession);
            }
            try (var store = new ReceiptLedger(path, SURFACE, false, () -> {})) {
                if (point == ReceiptLedger.CommitPoint.BEFORE_RENAME) {
                    assertFalse(store.killed()); // Negotiation was never allowed to begin.
                    store.beginSession();
                } else {
                    assertTrue(store.killed());
                    assertThrows(java.io.IOException.class, store::beginSession);
                }
            }
        }
    }
    @Test void failurePersistingRecoveryLatchCannotRestoreAuthority() throws Exception {
        for (var point : ReceiptLedger.CommitPoint.values()) {
            Path path = parent.resolve("recover-" + point);
            try (var store = new ReceiptLedger(path, SURFACE, true, () -> {})) { store.beginSession(); }
            var stops = new AtomicInteger();
            assertThrows(java.io.IOException.class, () -> new ReceiptLedger(path, SURFACE, false,
                    stops::incrementAndGet, phase -> { if (phase == point) throw new java.io.IOException("injected"); }));
            assertTrue(stops.get() >= 2);
            try (var store = new ReceiptLedger(path, SURFACE, false, () -> {})) {
                assertTrue(store.killed());
                assertThrows(java.io.IOException.class, store::beginSession);
            }
        }
    }
    @Test void exactBytesEpochSequenceAndKillSurviveReopen() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.advanceEpoch(BigInteger.ONE);
            assertEquals(BigInteger.ONE, store.nextSequence());
            store.record(act(), accepted(), BigInteger.TEN);
            store.kill();
        }
        var stops = new AtomicInteger();
        try (var store = new ReceiptLedger(ledger, SURFACE, false, stops::incrementAndGet)) {
            assertEquals(1, stops.get());
            assertEquals(BigInteger.ONE, store.epoch());
            assertEquals(BigInteger.TWO, store.nextSequence());
            assertTrue(store.killed());
            var found = store.lookup(COGLET, "2".repeat(32)).orElseThrow();
            assertArrayEquals(Codec.encodeProfile(act()), Codec.encodeProfile(found.envelope()));
            assertArrayEquals(Codec.encodeProfile(accepted()), Codec.encodeProfile(found.accept()));
            assertTrue(found.outcome().isEmpty());
            assertThrows(IllegalArgumentException.class, () -> store.advanceEpoch(BigInteger.ONE));
        }
    }
    @Test void recoveryRecordsUncertainFailureNeverReexecutes() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(act(), accepted(), BigInteger.TEN);
        }
        try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
            store.recoverPending(BigInteger.valueOf(120));
            var found = store.lookup(COGLET, "2".repeat(32)).orElseThrow();
            assertEquals("failed", found.outcome().orElseThrow().get("status"));
            assertArrayEquals(Codec.encodeProfile(act()), Codec.encodeProfile(found.envelope()));
            // Retention is relative to max(deadline, terminal time), not preparation.
            store.prune(BigInteger.valueOf(120).add(ReceiptLedger.RETENTION).subtract(BigInteger.ONE));
            assertTrue(store.lookup(COGLET, "2".repeat(32)).isPresent());
            store.prune(BigInteger.valueOf(120).add(ReceiptLedger.RETENTION));
            assertTrue(store.lookup(COGLET, "2".repeat(32)).isEmpty());
        }
    }
    @Test void returnedObjectsAreDetachedAndConflictingReuseIsRefused() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(act(), accepted(), BigInteger.TEN);
            store.lookup(COGLET, "2".repeat(32)).orElseThrow().envelope().clear();
            assertFalse(store.lookup(COGLET, "2".repeat(32)).orElseThrow().envelope().isEmpty());
            var changed = new HashMap<>(act());
            changed.put("body", Map.of("grant_epoch", 2, "action", ((Map<?,?>) act().get("body")).get("action")));
            assertThrows(IllegalArgumentException.class, () -> store.record(changed, accepted(), BigInteger.TEN));
            store.record(act(), accepted(), BigInteger.TEN);
        }
    }
    @Test void failedCommitStopsAndPoisonsEvenWhenRenameAlreadyHappened() throws Exception {
        for (var point : ReceiptLedger.CommitPoint.values()) {
            Path path = parent.resolve(point.name());
            var inject = new java.util.concurrent.atomic.AtomicBoolean();
            var stops = new AtomicInteger();
            try (var store = new ReceiptLedger(path, SURFACE, true, stops::incrementAndGet, phase -> {
                if (inject.get() && phase == point) throw new java.io.IOException("injected " + point);
            })) {
                store.advanceEpoch(BigInteger.ONE);
                inject.set(true);
                assertThrows(java.io.IOException.class, () -> store.advanceEpoch(BigInteger.TWO));
                assertEquals(2, stops.get());
                assertThrows(java.io.IOException.class, store::nextSequence);
                assertThrows(java.io.IOException.class, store::epoch);
            }
            try (var reopened = new ReceiptLedger(path, SURFACE, false, () -> {})) {
                assertEquals(point == ReceiptLedger.CommitPoint.BEFORE_RENAME ? BigInteger.ONE : BigInteger.TWO,
                        reopened.epoch());
            }
        }
    }
    @Test void terminalReceiptIsImmutableAndBoundToOriginalAction() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(act(), accepted(), BigInteger.TEN);
            var outcome = Map.<String,Object>of("id", "2".repeat(32), "correlation_id", FrameCodecTest.ID,
                    "status", "succeeded", "result", Map.of());
            store.finish(COGLET, "2".repeat(32), outcome, BigInteger.valueOf(99));
            store.finish(COGLET, "2".repeat(32), outcome, BigInteger.valueOf(100));
            assertThrows(IllegalArgumentException.class, () -> store.finish(COGLET, "2".repeat(32),
                    Map.of("id", "2".repeat(32), "correlation_id", "3".repeat(32), "status", "succeeded", "result", Map.of()),
                    BigInteger.valueOf(101)));
            store.recoverPending(BigInteger.valueOf(200));
            assertArrayEquals(Codec.encodeProfile(outcome), Codec.encodeProfile(
                    store.lookup(COGLET, "2".repeat(32)).orElseThrow().outcome().orElseThrow()));
        }
    }
    @Test void abruptJvmExitLeavesDurableRunningMarker() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {}
        var process = new ProcessBuilder(Path.of(System.getProperty("java.home"), "bin", "java").toString(),
                "-cp", System.getProperty("java.class.path"), ReceiptLedgerTest.class.getName(), ledger.toString(), "crash")
                .redirectErrorStream(true).start();
        try {
            assertTrue(process.waitFor(5, java.util.concurrent.TimeUnit.SECONDS));
            assertEquals(23, process.exitValue(), new String(process.getInputStream().readAllBytes()));
        } finally { process.destroyForcibly(); }
        try (var store = new ReceiptLedger(ledger, SURFACE, false, () -> {})) {
            assertTrue(store.killed());
            assertThrows(java.io.IOException.class, store::beginSession);
        }
    }
    public static void main(String[] args) throws Exception {
        if (args.length == 2 && args[1].equals("crash")) {
            var store = new ReceiptLedger(Path.of(args[0]), SURFACE, false, () -> {});
            store.beginSession();
            Runtime.getRuntime().halt(23);
        }
        try (var acquired = new ReceiptLedger(Path.of(args[0]), SURFACE, false, () -> {})) {
            System.out.println("ACQUIRED");
            System.exit(2);
        } catch (java.io.IOException e) {
            if (!e.getMessage().equals("ledger already owned")) throw e;
            System.out.println("LOCKED");
        }
    }
    void externalProcessMustRemainLocked() throws Exception {
        var process = new ProcessBuilder(Path.of(System.getProperty("java.home"), "bin", "java").toString(),
                "-cp", System.getProperty("java.class.path"), ReceiptLedgerTest.class.getName(), ledger.toString())
                .redirectErrorStream(true).start();
        try {
            assertTrue(process.waitFor(5, java.util.concurrent.TimeUnit.SECONDS));
            String output = new String(process.getInputStream().readAllBytes(), java.nio.charset.StandardCharsets.UTF_8);
            assertEquals(0, process.exitValue(), output);
            assertEquals("LOCKED", output.strip());
        } finally { process.destroyForcibly(); }
    }
    @Test void refusedSameJvmOpenerDoesNotReleaseExternalProcessLock() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            externalProcessMustRemainLocked();
            assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, () -> {}));
            externalProcessMustRemainLocked();
            Path alias = parent.resolve("alias");
            Files.createSymbolicLink(alias, parent);
            assertThrows(java.io.IOException.class, () -> new ReceiptLedger(alias.resolve("ledger"), SURFACE, false, () -> {}));
            externalProcessMustRemainLocked();
            Files.delete(alias);
        }
    }
    @Test void rejectionRetentionUsesDeadlineAndMissingStateNeverInitializes() throws Exception {
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(act(), Map.of("id", "2".repeat(32), "correlation_id", FrameCodecTest.ID,
                    "accepted", false, "error", Map.of("code", "permission_denied", "message", "denied", "retryable", false)), BigInteger.TEN);
            store.prune(BigInteger.valueOf(99).add(ReceiptLedger.RETENTION));
            assertTrue(store.lookup(COGLET, "2".repeat(32)).isPresent());
            store.prune(BigInteger.valueOf(100).add(ReceiptLedger.RETENTION));
            assertTrue(store.lookup(COGLET, "2".repeat(32)).isEmpty());
        }
        Files.delete(ledger.resolve("state.cbor"));
        assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, () -> {}));
        assertFalse(Files.exists(ledger.resolve("state.cbor")));
    }
    @Test void checksumMismatchRefusedAndInputMutationsDoNotAlterReceipt() throws Exception {
        var envelope = (Map<String,Object>) Codec.decodeProfile(Codec.encodeProfile(act()));
        var accept = new HashMap<>(accepted());
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            store.record(envelope, accept, BigInteger.TEN);
            envelope.clear(); accept.clear();
            assertArrayEquals(Codec.encodeProfile(act()), Codec.encodeProfile(
                    store.lookup(COGLET, "2".repeat(32)).orElseThrow().envelope()));
        }
        var wrapper = (Map<String,Object>) Codec.decodeProfile(Files.readAllBytes(ledger.resolve("state.cbor")));
        wrapper.put("sha256", "0".repeat(64));
        Files.write(ledger.resolve("state.cbor"), Codec.encodeProfile(wrapper));
        assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, () -> {}));
    }
    @Test void missingCorruptWrongIdentityAndConcurrentOpenFailClosed() throws Exception {
        var stops = new AtomicInteger();
        assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, stops::incrementAndGet));
        assertEquals(1, stops.get());
        try (var store = new ReceiptLedger(ledger, SURFACE, true, () -> {})) {
            assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, () -> {}));
            assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, true, () -> {}));
        }
        assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, "surface:" + "c".repeat(64), false, () -> {}));
        Files.write(ledger.resolve("state.cbor"), new byte[]{(byte)0xa0});
        assertThrows(java.io.IOException.class, () -> new ReceiptLedger(ledger, SURFACE, false, () -> {}));
    }
}
