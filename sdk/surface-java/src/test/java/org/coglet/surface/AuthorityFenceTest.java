package org.coglet.surface;

import static org.junit.jupiter.api.Assertions.*;
import java.math.BigInteger;
import java.util.*;
import java.util.concurrent.atomic.*;
import org.junit.jupiter.api.Test;

@org.junit.jupiter.api.Timeout(10)
class AuthorityFenceTest {
    static final String OWNER = ReceiptLedgerTest.COGLET;
    static final String OTHER = "coglet:" + "c".repeat(64);
    final AtomicReference<BigInteger> clock = new AtomicReference<>(BigInteger.ZERO);
    final AtomicInteger stops = new AtomicInteger();
    AuthorityFence fence(boolean killed) {
        return new AuthorityFence(Set.of(OWNER, OTHER), Map.of("expression.set_expression", Set.of("display.face"),
                "speech.config", Set.of()), BigInteger.ZERO, killed, clock::get, stops::incrementAndGet);
    }
    AuthorityFence.Grant grant(String owner, Set<String> ops, Set<String> resources) {
        return new AuthorityFence.Grant(owner, ops, resources, BigInteger.valueOf(10_000_000));
    }
    void install(AuthorityFence f, int epoch) {
        f.replace(BigInteger.valueOf(epoch), List.of(grant(OWNER, Set.of("expression.set_expression", "speech.config"), Set.of("display.face"))));
    }
    @Test void resourceOperationOwnerEpochAndExclusiveDeadline() throws Exception {
        var f = fence(false); install(f, 1);
        var effects = new AtomicInteger();
        var token = f.begin(OWNER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of("speech.config"));
        assertEquals(1, f.effect(token, effects::incrementAndGet));
        assertThrows(AuthorityFence.Denied.class, () -> f.begin(OTHER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of()));
        assertThrows(AuthorityFence.Denied.class, () -> f.begin(OWNER, "expression.set_expression", BigInteger.TWO, BigInteger.valueOf(100), Set.of()));
        clock.set(BigInteger.valueOf(100));
        assertThrows(AuthorityFence.Denied.class, () -> f.effect(token, effects::incrementAndGet));
        assertEquals(1, effects.get());
    }
    @Test void revokeReenableAndReconstructionCannotReviveToken() throws Exception {
        var f = fence(false); install(f, 1);
        var token = f.begin(OWNER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        f.replace(BigInteger.TWO, List.of());
        install(f, 3);
        assertThrows(AuthorityFence.Denied.class, () -> f.effect(token, () -> fail()));
        var newFence = fence(false); install(newFence, 1);
        assertThrows(AuthorityFence.Denied.class, () -> newFence.effect(token, () -> fail()));
        var live = f.begin(OWNER, "expression.set_expression", BigInteger.valueOf(3), BigInteger.valueOf(100), Set.of());
        assertEquals("ok", f.effect(live, () -> "ok"));
    }
    @Test void expiryDuringNativeStopRetainsHighWaterButNoAuthority() {
        var expireOnStop = new AtomicBoolean();
        var f = new AuthorityFence(Set.of(OWNER), Map.of("speech.config", Set.of()),
                BigInteger.ZERO, false, clock::get, () -> {
                    stops.incrementAndGet();
                    if (expireOnStop.get()) clock.set(BigInteger.valueOf(10_000_000));
                });
        f.replace(BigInteger.ONE, List.of(grant(OWNER, Set.of("speech.config"), Set.of())));
        var old = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        expireOnStop.set(true);
        assertThrows(AuthorityFence.Denied.class, () -> f.replace(BigInteger.TWO,
                List.of(grant(OWNER, Set.of("speech.config"), Set.of()))));
        assertThrows(AuthorityFence.Denied.class, () -> f.check(old));
        assertThrows(IllegalArgumentException.class, () -> f.replace(BigInteger.TWO, List.of()));
        expireOnStop.set(false);
        f.replace(BigInteger.valueOf(3), List.of(new AuthorityFence.Grant(OWNER, Set.of("speech.config"),
                Set.of(), BigInteger.valueOf(20_000_000))));
        f.check(f.begin(OWNER, "speech.config", BigInteger.valueOf(3), BigInteger.valueOf(11_000_000), Set.of()));
    }
    @Test void invalidSnapshotHasNoPartialAuthorityMutation() throws Exception {
        var f = fence(false); install(f, 1);
        var token = f.begin(OWNER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        int before = stops.get();
        assertThrows(IllegalArgumentException.class, () -> f.replace(BigInteger.TWO, List.of(
                grant(OWNER, Set.of("expression.set_expression"), Set.of("display.face")),
                grant(OTHER, Set.of("expression.set_expression"), Set.of("display.face")))));
        assertThrows(IllegalArgumentException.class, () -> f.replace(BigInteger.TWO, List.of(
                grant(OWNER, Set.of("unknown.operation"), Set.of()))));
        assertEquals(before, stops.get());
        assertEquals("still live", f.effect(token, () -> "still live"));
    }
    @Test void killIsLatchedAndPostEncodingCheckRejectsLateOrStaleObservation() throws Exception {
        var f = fence(false); install(f, 1);
        var token = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        clock.set(BigInteger.valueOf(100));
        assertThrows(AuthorityFence.Denied.class, () -> f.check(token));
        clock.set(BigInteger.valueOf(101));
        f.kill();
        assertThrows(AuthorityFence.Denied.class, () -> f.replace(BigInteger.TWO, List.of()));
        assertThrows(AuthorityFence.Denied.class, () -> f.effect(token, () -> fail()));
        assertTrue(stops.get() >= 2);
        var restored = fence(true);
        assertThrows(AuthorityFence.Denied.class, () -> restored.replace(BigInteger.ONE, List.of()));
    }
    @Test void missingConjoinedOperationAndResourceAreDenied() {
        var f = fence(false);
        f.replace(BigInteger.ONE, List.of(grant(OWNER, Set.of("expression.set_expression"), Set.of())));
        assertThrows(AuthorityFence.Denied.class, () -> f.begin(OWNER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of()));
        assertThrows(AuthorityFence.Denied.class, () -> f.begin(OWNER, "expression.set_expression", BigInteger.ONE, BigInteger.valueOf(100), Set.of("speech.config")));
    }
    @Test void nativeKillDoesNotWaitForBlockedReceiptCommit() throws Exception {
        var root = java.nio.file.Files.createTempDirectory(java.nio.file.Path.of(System.getenv().getOrDefault(
                "TMPDIR", System.getProperty("java.io.tmpdir"))), "cg-fence-");
        var block = new AtomicBoolean();
        var entered = new java.util.concurrent.CountDownLatch(1);
        var release = new java.util.concurrent.CountDownLatch(1);
        var f = fence(false);
        try (var ledger = new ReceiptLedger(root.resolve("ledger"), ReceiptLedgerTest.SURFACE, true, f::disconnect, point -> {
            if (block.get() && point == ReceiptLedger.CommitPoint.BEFORE_RENAME) {
                entered.countDown();
                try { release.await(); }
                catch (InterruptedException e) { Thread.currentThread().interrupt(); throw new java.io.IOException(e); }
            }
        }); var executor = java.util.concurrent.Executors.newSingleThreadExecutor()) {
            ledger.advanceEpoch(BigInteger.ONE); install(f, 1);
            var token = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
            block.set(true);
            var writer = executor.submit(() -> {
                try { ledger.advanceEpoch(BigInteger.TWO); }
                catch (java.io.IOException e) { throw new RuntimeException(e); }
            });
            try {
                assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS));
                int before = stops.get();
                f.kill();
                assertTrue(stops.get() > before);
                assertFalse(writer.isDone());
                assertThrows(AuthorityFence.Denied.class, () -> f.effect(token, () -> fail()));
            } finally { release.countDown(); }
            writer.get(3, java.util.concurrent.TimeUnit.SECONDS);
        } finally {
            try (var paths = java.nio.file.Files.walk(root)) {
                for (var path : paths.sorted(Comparator.reverseOrder()).toList()) java.nio.file.Files.deleteIfExists(path);
            }
        }
    }
    @Test void nativeEffectThatCrossesDeadlineCannotPublishTimelySuccess() throws Exception {
        var f = fence(false); install(f, 1);
        var token = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        assertEquals("actual native result retained for audit", f.effect(token, () -> {
            clock.set(BigInteger.valueOf(100));
            return "actual native result retained for audit";
        }));
        assertThrows(AuthorityFence.Denied.class, () -> f.check(token));
    }
    @Test void invalidClockStopsRatherThanResumingOnNextGoodSample() {
        var f = fence(false); install(f, 1);
        var token = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        clock.set(BigInteger.valueOf(-1));
        assertThrows(FrameCodec.ProtocolException.class, () -> f.check(token));
        clock.set(BigInteger.TEN);
        assertThrows(AuthorityFence.Denied.class, () -> f.check(token));
    }
    @Test void clockRollbackStopsRatherThanRestoringAuthority() {
        var f = fence(false); install(f, 1);
        clock.set(BigInteger.TEN);
        var token = f.begin(OWNER, "speech.config", BigInteger.ONE, BigInteger.valueOf(100), Set.of());
        clock.set(BigInteger.ONE);
        assertThrows(FrameCodec.ProtocolException.class, () -> f.check(token));
        clock.set(BigInteger.valueOf(20));
        assertThrows(AuthorityFence.Denied.class, () -> f.check(token));
    }
}
