package org.coglet.surface;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.channels.*;
import java.nio.file.*;
import java.nio.file.attribute.*;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.*;

/**
 * Bounded, exclusively owned durable surface receipt store. It does not authorize
 * device effects: session/native-owner fencing must precede record/dispatch.
 * Every update forces a new snapshot, atomically replaces the state file, then
 * forces its directory before publishing the new in-memory state. I/O uncertainty
 * poisons this instance; never continue from an assumed rollback.
 *
 * Creation is a trusted one-time provisioning operation for a NEW surface
 * identity. The supervisor must retain its identity anchor outside this directory
 * and never recreate a deleted store under the old identity. Reopen never creates
 * a missing directory/state. Same-UID hostile filesystem edits are out of scope.
 */
public final class ReceiptLedger implements AutoCloseable {
    public static final BigInteger RETENTION = BigInteger.valueOf(86_400_000_000L);
    private static final int MAX_BYTES = 32 * 1024 * 1024;
    private static final int MAX_RECEIPTS = 4096;
    // POSIX locks are process-scoped: merely closing a second descriptor can
    // release the first channel's lock. Reject same-JVM aliases before open().
    private static final Set<Object> JVM_OWNERS = new HashSet<>();
    private Object ownershipKey;
    private final Path directory;
    private final String identity;
    private final Runnable stop;
    enum CommitPoint { BEFORE_RENAME, AFTER_RENAME, AFTER_DIRECTORY_FORCE }
    @FunctionalInterface interface CommitProbe { void check(CommitPoint point) throws IOException; }
    private final CommitProbe commitProbe;
    private FileChannel lockChannel;
    private FileLock lock;
    private Map<String,Object> state;
    private boolean poisoned;
    private boolean closed;

    public record Receipt(Map<String,Object> envelope, Map<String,Object> accept,
                          Optional<Map<String,Object>> outcome) {}

    public ReceiptLedger(Path directory, String identity, boolean create, Runnable stop) throws IOException {
        this(directory, identity, create, stop, point -> {});
    }
    ReceiptLedger(Path directory, String identity, boolean create, Runnable stop, CommitProbe commitProbe) throws IOException {
        this.stop = Objects.requireNonNull(stop);
        this.commitProbe = Objects.requireNonNull(commitProbe);
        stop.run(); // No ledger/config failure may leave uncertain work running.
        Codec.validate("common::surface-id", identity);
        this.directory = directory.toAbsolutePath();
        this.identity = identity;
        try {
            if (create) Files.createDirectory(this.directory, PosixFilePermissions.asFileAttribute(
                    PosixFilePermissions.fromString("rwx------")));
            var attrs = Files.readAttributes(this.directory, PosixFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
            if (!attrs.isDirectory() || !attrs.permissions().equals(PosixFilePermissions.fromString("rwx------"))
                    || !attrs.owner().equals(this.directory.getFileSystem().getUserPrincipalLookupService()
                        .lookupPrincipalByName(System.getProperty("user.name"))))
                throw new IOException("ledger directory is not private and owned");
            Object directoryKey = attrs.fileKey();
            if (directoryKey == null) throw new IOException("ledger file identity unavailable");
            synchronized (JVM_OWNERS) {
                if (!JVM_OWNERS.add(directoryKey)) throw new IOException("ledger already owned");
                ownershipKey = directoryKey;
            }
            Path lockPath = this.directory.resolve("lock");
            lockChannel = FileChannel.open(lockPath, Set.of(StandardOpenOption.CREATE, StandardOpenOption.WRITE,
                    LinkOption.NOFOLLOW_LINKS), PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")));
            try { lock = lockChannel.tryLock(); }
            catch (OverlappingFileLockException e) { throw new IOException("ledger already owned", e); }
            if (lock == null) throw new IOException("ledger already owned");
            if (create) {
                state = new LinkedHashMap<>(Map.of("format", BigInteger.ONE, "identity", identity,
                        "epoch", BigInteger.ZERO, "sequence", BigInteger.ZERO, "killed", false, "receipts", List.of()));
                persist(state);
                // Publish the newly created directory entry durably too, before
                // provisioning can return success to its external identity anchor.
                try (var parent = FileChannel.open(this.directory.getParent(), StandardOpenOption.READ)) { parent.force(true); }
            } else {
                Path file = this.directory.resolve("state.cbor");
                var fileAttrs = Files.readAttributes(file, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
                if (!fileAttrs.isRegularFile() || fileAttrs.size() > MAX_BYTES) throw new IOException("invalid ledger file");
                try (var in = FileChannel.open(file, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS)) {
                    ByteBuffer data = ByteBuffer.allocate(Math.toIntExact(fileAttrs.size()));
                    while (data.hasRemaining()) if (in.read(data) < 0) throw new IOException("truncated ledger");
                    if (in.read(ByteBuffer.allocate(1)) != -1) throw new IOException("growing ledger");
                    Map<String,Object> wrapper = map(Codec.decodeProfile(data.array()));
                    if (!wrapper.keySet().equals(Set.of("state", "sha256"))) throw new IOException("invalid ledger wrapper");
                    Map<String,Object> loaded = map(wrapper.get("state"));
                    if (!hash(Codec.encodeProfile(loaded)).equals(wrapper.get("sha256"))) throw new IOException("ledger checksum");
                    validateState(loaded);
                    state = loaded;
                }
            }
        } catch (IOException | RuntimeException e) {
            close();
            if (e instanceof IOException io) throw io;
            throw new IOException("invalid receipt ledger", e);
        }
    }

    @SuppressWarnings("unchecked")
    private static Map<String,Object> map(Object value) {
        if (!(value instanceof Map<?,?>)) throw new IllegalArgumentException("expected record");
        return (Map<String,Object>) value;
    }
    private static BigInteger number(Object value) {
        Codec.validate("common::uint64", value);
        return new BigInteger(value.toString());
    }
    private static Map<String,Object> detached(Map<String,Object> value) {
        return map(Codec.decodeProfile(Codec.encodeProfile(value)));
    }
    private static String hash(byte[] raw) {
        try { return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(raw)); }
        catch (NoSuchAlgorithmException e) { throw new AssertionError(e); }
    }
    private void check() throws IOException {
        if (closed || poisoned) throw new IOException("ledger closed or uncertain; reopen required");
    }
    @SuppressWarnings("unchecked")
    private static List<Object> records(Map<String,Object> value) { return (List<Object>) value.get("receipts"); }
    private static Map<String,Object> action(Map<String,Object> envelope) {
        Codec.validate(FrameCodec.WIRE_RULE, envelope);
        if (!"act".equals(envelope.get("type"))) throw new IllegalArgumentException("expected act envelope");
        return map(map(envelope.get("body")).get("action"));
    }
    private static void matchesAction(Map<String,Object> action, Map<String,Object> receipt) {
        if (!action.get("id").equals(receipt.get("id"))
                || !action.get("correlation_id").equals(receipt.get("correlation_id")))
            throw new IllegalArgumentException("receipt action binding");
    }
    private void validateState(Map<String,Object> value) {
        if (!value.keySet().equals(Set.of("format", "identity", "epoch", "sequence", "killed", "receipts"))
                || !BigInteger.ONE.equals(value.get("format")) || !identity.equals(value.get("identity"))
                || !(value.get("killed") instanceof Boolean) || !(value.get("receipts") instanceof List<?>))
            throw new IllegalArgumentException("ledger identity or format");
        number(value.get("epoch")); number(value.get("sequence"));
        if (records(value).size() > MAX_RECEIPTS) throw new IllegalArgumentException("receipt capacity");
        var keys = new HashSet<String>();
        for (Object raw : records(value)) {
            Map<String,Object> record = map(raw);
            if (!record.keySet().equals(Set.of("envelope", "accept", "outcome", "terminal")))
                throw new IllegalArgumentException("invalid receipt fields");
            var a = action(map(record.get("envelope")));
            String key = a.get("coglet") + "/" + a.get("id");
            if (!keys.add(key)) throw new IllegalArgumentException("duplicate receipt");
            var accept = map(record.get("accept"));
            Codec.validate("common::action-accept", accept); matchesAction(a, accept);
            Object terminal = record.get("terminal");
            Object outcome = record.get("outcome");
            if (outcome != null) {
                Codec.validate("common::action-outcome", outcome); matchesAction(a, map(outcome));
                if (!Boolean.TRUE.equals(accept.get("accepted")) || terminal == null)
                    throw new IllegalArgumentException("outcome without acceptance/terminal");
            } else if (Boolean.TRUE.equals(accept.get("accepted")) && terminal != null) {
                throw new IllegalArgumentException("accepted terminal lacks outcome");
            } else if (Boolean.FALSE.equals(accept.get("accepted")) && terminal == null) {
                throw new IllegalArgumentException("rejection lacks terminal time");
            }
            if (terminal != null) number(terminal);
        }
    }

    private void persist(Map<String,Object> candidate) throws IOException {
        validateState(candidate);
        byte[] raw = Codec.encodeProfile(Map.of("state", candidate, "sha256", hash(Codec.encodeProfile(candidate))));
        if (raw.length > MAX_BYTES) throw new IOException("receipt byte capacity");
        Path temp = directory.resolve("state-" + UUID.randomUUID() + ".new");
        try {
            try (var out = FileChannel.open(temp, Set.of(StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE),
                    PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")))) {
                ByteBuffer bytes = ByteBuffer.wrap(raw);
                while (bytes.hasRemaining()) out.write(bytes);
                out.force(true);
            }
            commitProbe.check(CommitPoint.BEFORE_RENAME);
            Files.move(temp, directory.resolve("state.cbor"), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
            commitProbe.check(CommitPoint.AFTER_RENAME);
            try (var dir = FileChannel.open(directory, StandardOpenOption.READ)) { dir.force(true); }
            commitProbe.check(CommitPoint.AFTER_DIRECTORY_FORCE);
            state = candidate;
        } catch (IOException | RuntimeException e) {
            poisoned = true;
            try { stop.run(); } catch (RuntimeException stopFailure) { e.addSuppressed(stopFailure); }
            throw e;
        } finally {
            Files.deleteIfExists(temp);
        }
    }
    public synchronized BigInteger epoch() throws IOException { check(); return number(state.get("epoch")); }
    public synchronized boolean killed() throws IOException { check(); return Boolean.TRUE.equals(state.get("killed")); }
    public synchronized void advanceEpoch(BigInteger epoch) throws IOException {
        check(); number(epoch);
        if (epoch.compareTo(epoch()) <= 0) throw new IllegalArgumentException("epoch must increase");
        var next = detached(state); next.put("epoch", epoch); persist(next);
    }
    public synchronized BigInteger nextSequence() throws IOException {
        check(); BigInteger sequence = number(state.get("sequence")).add(BigInteger.ONE); number(sequence);
        var next = detached(state); next.put("sequence", sequence); persist(next); return sequence;
    }
    public synchronized void kill() throws IOException {
        check(); var next = detached(state); next.put("killed", true); persist(next);
    }
    private static Map<String,Object> find(Map<String,Object> state, String coglet, String id) {
        for (Object raw : records(state)) {
            var record = map(raw); var a = action(map(record.get("envelope")));
            if (coglet.equals(a.get("coglet")) && id.equals(a.get("id"))) return record;
        }
        return null;
    }
    public synchronized Optional<Receipt> lookup(String coglet, String id) throws IOException {
        check(); var record = find(state, coglet, id);
        if (record == null) return Optional.empty();
        return Optional.of(new Receipt(detached(map(record.get("envelope"))), detached(map(record.get("accept"))),
                Optional.ofNullable(record.get("outcome")).map(x -> detached(map(x)))));
    }
    /** Persist before acknowledging OR starting an effect. Exact existing bytes are a no-op. */
    public synchronized void record(Map<String,Object> envelope, Map<String,Object> accept, BigInteger now) throws IOException {
        check(); number(now); var a = action(envelope);
        Codec.validate("common::action-accept", accept); matchesAction(a, accept);
        var old = find(state, (String) a.get("coglet"), (String) a.get("id"));
        if (old != null) {
            if (!Arrays.equals(Codec.encodeProfile(envelope), Codec.encodeProfile(old.get("envelope")))
                    || !Arrays.equals(Codec.encodeProfile(accept), Codec.encodeProfile(old.get("accept"))))
                throw new IllegalArgumentException("conflicting action ID");
            return;
        }
        if (records(state).size() >= MAX_RECEIPTS) throw new IOException("receipt capacity");
        var next = detached(state); var record = new LinkedHashMap<String,Object>();
        record.put("envelope", envelope); record.put("accept", accept); record.put("outcome", null);
        record.put("terminal", Boolean.TRUE.equals(accept.get("accepted")) ? null : now);
        records(next).add(record); persist(detached(next));
    }
    public synchronized void finish(String coglet, String id, Map<String,Object> outcome, BigInteger now) throws IOException {
        check(); number(now); Codec.validate("common::action-outcome", outcome);
        var next = detached(state); var record = find(next, coglet, id);
        if (record == null || !Boolean.TRUE.equals(map(record.get("accept")).get("accepted")))
            throw new IllegalArgumentException("outcome without admitted action");
        matchesAction(action(map(record.get("envelope"))), outcome);
        if (record.get("outcome") != null) {
            if (!Arrays.equals(Codec.encodeProfile(outcome), Codec.encodeProfile(record.get("outcome"))))
                throw new IllegalArgumentException("conflicting terminal outcome");
            return;
        }
        record.put("outcome", outcome); record.put("terminal", now); persist(detached(next));
    }
    /** Call only after the independent native stop reflex; never replay pending effects. */
    public synchronized void recoverPending(BigInteger now) throws IOException {
        check(); number(now); var next = detached(state);
        for (Object raw : records(next)) {
            var record = map(raw);
            if (Boolean.TRUE.equals(map(record.get("accept")).get("accepted")) && record.get("outcome") == null) {
                var a = action(map(record.get("envelope")));
                record.put("outcome", Map.of("id", a.get("id"), "correlation_id", a.get("correlation_id"),
                        "status", "failed", "error", Map.of("code", "unavailable", "message", "uncertain native effect", "retryable", false)));
                record.put("terminal", now);
            }
        }
        persist(next);
    }
    public synchronized void prune(BigInteger now) throws IOException {
        check(); number(now); var next = detached(state);
        records(next).removeIf(raw -> {
            var record = map(raw);
            if (record.get("terminal") == null) return false;
            BigInteger deadline = number(action(map(record.get("envelope"))).get("deadline"));
            BigInteger until = deadline.max(number(record.get("terminal"))).add(RETENTION);
            return now.compareTo(until) >= 0;
        });
        persist(next);
    }
    @Override public synchronized void close() throws IOException {
        if (closed) return;
        closed = true;
        try { if (lock != null) lock.release(); }
        finally {
            try { if (lockChannel != null) lockChannel.close(); }
            finally {
                synchronized (JVM_OWNERS) {
                    if (ownershipKey != null) JVM_OWNERS.remove(ownershipKey);
                }
            }
        }
    }
}
