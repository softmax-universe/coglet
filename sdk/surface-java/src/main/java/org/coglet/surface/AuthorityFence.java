package org.coglet.surface;

import java.math.BigInteger;
import java.util.*;
import java.util.concurrent.locks.ReentrantLock;
import java.util.function.Supplier;

/**
 * Memory/native-owner authority fence, deliberately independent of the receipt
 * store and transport queues. This is not a surface session or policy source.
 * The supervisor must durably advance its grant epoch BEFORE publishing the new
 * snapshot here and must first disconnect old authority before blocking on disk.
 * Native effect/stop callbacks MUST be bounded and use this same owner fence;
 * never hold it across disk, network, telemetry or arbitrary control callbacks.
 */
public final class AuthorityFence {
    public record Grant(String coglet, Set<String> operations, Set<String> resources, BigInteger expires) {
        public Grant {
            Objects.requireNonNull(coglet); Objects.requireNonNull(expires);
            operations = Set.copyOf(operations); resources = Set.copyOf(resources);
        }
    }
    public static final class Denied extends IllegalArgumentException {
        private final String code;
        private Denied(String code, String message) { super(message); this.code = code; }
        public String code() { return code; }
    }
    @FunctionalInterface public interface Effect<T> { T run() throws Exception; }

    /** Opaque instance-bound token: reconstructing a fence never revives old callbacks. */
    public static final class Token {
        private final AuthorityFence owner;
        private final String coglet;
        private final String operation;
        private final Set<String> required;
        private final BigInteger epoch;
        private final BigInteger deadline;
        private final BigInteger generation;
        private Token(AuthorityFence owner, String coglet, String operation, Set<String> required,
                      BigInteger epoch, BigInteger deadline, BigInteger generation) {
            this.owner = owner; this.coglet = coglet; this.operation = operation; this.required = required;
            this.epoch = epoch; this.deadline = deadline; this.generation = generation;
        }
        public String coglet() { return coglet; }
        public String operation() { return operation; }
        public BigInteger epoch() { return epoch; }
        public BigInteger deadline() { return deadline; }
        public BigInteger generation() { return generation; }
    }

    private final ReentrantLock nativeOwner = new ReentrantLock();
    private final Set<String> permitted;
    private final Map<String,Set<String>> operationResources;
    private final Set<String> resources;
    private final Supplier<BigInteger> clock;
    private final Runnable nativeStop;
    private Map<String,Grant> grants = Map.of();
    private BigInteger epoch;
    private BigInteger generation = BigInteger.ZERO;
    private BigInteger lastClock = BigInteger.ZERO;
    private volatile boolean killed;
    private volatile boolean stopped = true;

    public AuthorityFence(Set<String> permitted, Map<String,Set<String>> operationResources,
                          BigInteger retainedEpoch, boolean retainedKill,
                          Supplier<BigInteger> clock, Runnable nativeStop) {
        this.nativeStop = Objects.requireNonNull(nativeStop);
        nativeStop.run();
        this.clock = Objects.requireNonNull(clock);
        this.permitted = Set.copyOf(permitted);
        var copy = new LinkedHashMap<String,Set<String>>();
        var knownResources = new HashSet<String>();
        for (String coglet : permitted) Codec.validate("common::coglet-id", coglet);
        operationResources.forEach((name, required) -> {
            Codec.validate("common::capability-name", name);
            for (String resource : required) Codec.validate("common::capability-name", resource);
            var immutable = Set.copyOf(required);
            copy.put(name, immutable); knownResources.addAll(immutable);
        });
        this.operationResources = Map.copyOf(copy);
        this.resources = Set.copyOf(knownResources);
        uint(retainedEpoch); epoch = retainedEpoch; killed = retainedKill;
    }
    private static void uint(BigInteger value) { Codec.validate("common::uint64", value); }
    private static Denied denied(String message) { return new Denied("permission_denied", message); }
    private BigInteger sample() {
        BigInteger now;
        try { now = clock.get(); uint(now); }
        catch (RuntimeException e) {
            killed = true; stopped = true; generation = generation.add(BigInteger.ONE);
            nativeStop.run();
            throw new FrameCodec.ProtocolException("invalid surface clock", e);
        }
        if (now.compareTo(lastClock) < 0) {
            killed = true; stopped = true; generation = generation.add(BigInteger.ONE);
            nativeStop.run();
            throw new FrameCodec.ProtocolException("surface clock rollback");
        }
        lastClock = now;
        return now;
    }

    /**
     * Full trusted snapshot. Structural validation leaves old authority unchanged;
     * expiry/kill/clock failure after native stop begins instead leaves it stopped.
     */
    public void replace(BigInteger nextEpoch, List<Grant> snapshot) {
        uint(nextEpoch);
        snapshot = List.copyOf(snapshot);
        nativeOwner.lock();
        try {
            if (killed) throw denied("kill is latched");
            if (nextEpoch.compareTo(epoch) <= 0) throw new IllegalArgumentException("epoch must increase");
            BigInteger now = sample();
            var next = new HashMap<String,Grant>();
            var owned = new HashSet<String>();
            for (Grant grant : snapshot) {
                uint(grant.expires());
                if (!permitted.contains(grant.coglet()) || next.putIfAbsent(grant.coglet(), grant) != null
                        || !operationResources.keySet().containsAll(grant.operations())
                        || !resources.containsAll(grant.resources()) || grant.expires().compareTo(now) <= 0)
                    throw new IllegalArgumentException("invalid grant snapshot");
                for (String resource : grant.resources()) {
                    if (!owned.add(resource)) throw new IllegalArgumentException("duplicate resource owner");
                }
            }
            stopped = true;
            generation = generation.add(BigInteger.ONE);
            epoch = nextEpoch; // Retain the durably committed high-water even if stop/expiry refuses installation.
            grants = Map.of();
            nativeStop.run();
            // Stop may be a bounded native transaction, not a clock freeze.
            now = sample();
            for (Grant grant : next.values()) {
                if (grant.expires().compareTo(now) <= 0) throw denied("grant expired while replacing authority");
            }
            if (killed) throw denied("kill is latched");
            grants = Map.copyOf(next);
            stopped = false;
        } finally { nativeOwner.unlock(); }
    }

    /** Admission preflight only; effect() performs the final atomic native check again. */
    public Token begin(String coglet, String operation, BigInteger requestEpoch, BigInteger deadline,
                       Set<String> additionallyRequiredOperations) {
        uint(requestEpoch); uint(deadline);
        var required = new HashSet<>(additionallyRequiredOperations); required.add(operation);
        nativeOwner.lock();
        try {
            BigInteger now = sample();
            if (deadline.compareTo(now.add(BigInteger.valueOf(60_000_000))) > 0)
                throw new Denied("invalid_argument", "deadline exceeds sixty seconds");
            var token = new Token(this, coglet, operation, Set.copyOf(required), requestEpoch, deadline, generation);
            verify(token, now);
            return token;
        } finally { nativeOwner.unlock(); }
    }
    private void verify(Token token, BigInteger now) {
        if (token == null || token.owner != this || killed || stopped || !token.epoch.equals(epoch)
                || !token.generation.equals(generation)) throw denied("stale native authority");
        if (now.compareTo(token.deadline) >= 0) throw new Denied("deadline_exceeded", "exclusive deadline reached");
        Grant grant = grants.get(token.coglet);
        if (grant == null || now.compareTo(grant.expires()) >= 0 || !grant.operations().containsAll(token.required))
            throw denied("missing or expired operation grant");
        for (String operation : token.required) {
            Set<String> needed = operationResources.get(operation);
            if (needed == null || !grant.resources().containsAll(needed)) throw denied("missing resource grant");
        }
    }
    /** Recheck after passive sampling/encoding and before queuing private output. */
    public void check(Token token) {
        nativeOwner.lock();
        try { verify(token, sample()); }
        finally { nativeOwner.unlock(); }
    }
    /**
     * Atomic final authority check + bounded native transaction. A result is not
     * publication: check again afterward, and retain any late actual effect as
     * audit rather than claiming a rollback or a timely terminal success.
     */
    public <T> T effect(Token token, Effect<T> effect) throws Exception {
        nativeOwner.lock();
        try { verify(token, sample()); return effect.run(); }
        finally { nativeOwner.unlock(); }
    }
    /** Independent native link-loss/revoke reflex: no disk or control-queue access. */
    public void disconnect() {
        stopped = true;
        nativeOwner.lock();
        try {
            stopped = true;
            generation = generation.add(BigInteger.ONE); grants = Map.of();
            nativeStop.run();
        } finally { nativeOwner.unlock(); }
    }
    /** Latched until a new explicitly provisioned supervisor/fence instance. */
    public void kill() {
        killed = true;
        disconnect();
    }
}
