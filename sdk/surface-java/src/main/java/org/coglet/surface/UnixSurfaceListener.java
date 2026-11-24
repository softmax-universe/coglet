package org.coglet.surface;

import java.io.IOException;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.ByteBuffer;
import java.nio.channels.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.nio.file.attribute.*;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.Semaphore;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;
import jdk.net.ExtendedSocketOptions;

/**
 * Surface-owned AF_UNIX listener. The directory must already be private and owned
 * by the invoking user. OS peer credentials, not a hello claim, select the trusted
 * runtime user. Same-UID hostile code is outside the frozen isolation boundary.
 * This class handles transport only: a Surface session must enforce lifecycle,
 * grants, selected operation schemas and durable receipts before device access.
 */
public final class UnixSurfaceListener implements AutoCloseable {
    private final Path path;
    private final UserPrincipal runtimeUser;
    private final Runnable stop;
    private final ServerSocketChannel server;
    private final Object socketKey;
    private boolean closed;

    public UnixSurfaceListener(Path path, UserPrincipal runtimeUser, Runnable stop) throws IOException {
        this.path = path.toAbsolutePath();
        this.runtimeUser = Objects.requireNonNull(runtimeUser);
        this.stop = Objects.requireNonNull(stop);
        if (this.path.toString().getBytes(StandardCharsets.UTF_8).length > 100)
            throw new IllegalArgumentException("Unix socket path exceeds portable bound");
        Path parent = this.path.getParent();
        var currentUser = parent.getFileSystem().getUserPrincipalLookupService()
                .lookupPrincipalByName(System.getProperty("user.name"));
        var rootUser = parent.getFileSystem().getUserPrincipalLookupService().lookupPrincipalByName("root");
        var ancestors = new ArrayList<Path>();
        for (Path p = parent; p != null; p = p.getParent()) ancestors.add(p);
        Collections.reverse(ancestors);
        boolean privatePrefix = false;
        for (Path p : ancestors) {
            if (Files.isSymbolicLink(p)) throw new SecurityException("symlink socket ancestor");
            var ancestor = Files.readAttributes(p, PosixFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
            int mode = (Integer) Files.getAttribute(p, "unix:mode", LinkOption.NOFOLLOW_LINKS);
            if (!ancestor.isDirectory() || (!ancestor.owner().equals(currentUser) && !ancestor.owner().equals(rootUser)))
                throw new SecurityException("untrusted socket ancestor");
            if (!privatePrefix) requireNonreplaceableMode(mode);
            // Below a trusted private directory other UIDs cannot reach/replace
            // components even if an interior directory grants group write.
            if (ancestor.owner().equals(currentUser) && (mode & 0077) == 0) privatePrefix = true;
        }
        var attrs = Files.readAttributes(parent, PosixFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
        if (!attrs.isDirectory() || !attrs.owner().equals(currentUser)
                || !attrs.permissions().equals(PosixFilePermissions.fromString("rwx------")))
            throw new SecurityException("socket directory must be current-user owned mode 0700");
        if (Files.exists(this.path, LinkOption.NOFOLLOW_LINKS)) throw new FileAlreadyExistsException(this.path.toString());
        server = ServerSocketChannel.open(StandardProtocolFamily.UNIX);
        Object key = null;
        boolean bound = false;
        try {
            server.bind(UnixDomainSocketAddress.of(this.path));
            bound = true;
            key = Files.readAttributes(this.path, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS).fileKey();
            if (key == null) throw new IOException("socket file identity unavailable");
            Files.setPosixFilePermissions(this.path, PosixFilePermissions.fromString("rw-------"));
        } catch (IOException | RuntimeException e) {
            server.close();
            if (bound && key != null && sameFileKey(key)) Files.deleteIfExists(this.path);
            throw e;
        }
        socketKey = key;
    }

    static void requireNonreplaceableMode(int mode) {
        if ((mode & 0022) != 0 && (mode & 01000) == 0)
            throw new SecurityException("replaceable socket ancestor");
    }

    /** Blocking accept on the supervisor control thread; no device data precedes credential checks. */
    public Connection accept() throws IOException {
        SocketChannel channel = server.accept();
        try {
            var peer = channel.getOption(ExtendedSocketOptions.SO_PEERCRED);
            if (peer == null || !runtimeUser.equals(peer.user())) throw new SecurityException("unbound runtime peer");
            return new Connection(channel, peer.user(), stop);
        } catch (IOException | RuntimeException e) {
            try { stop.run(); } finally { channel.close(); }
            throw e;
        }
    }

    private boolean sameFileKey(Object key) throws IOException {
        return Files.exists(path, LinkOption.NOFOLLOW_LINKS) && key.equals(
                Files.readAttributes(path, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS).fileKey());
    }
    @Override public void close() throws IOException {
        if (closed) return;
        closed = true;
        server.close();
        if (sameFileKey(socketKey)) Files.deleteIfExists(path);
    }

    /**
     * One poll owner, concurrent bounded enqueue/close. Native stop never waits on
     * the writer queue or selector. A fatal decoder, direction, callback or I/O
     * failure closes the link and invokes the stop reflex exactly once.
     */
    public static final class Connection implements AutoCloseable {
        private static final class ClosedDispatch extends RuntimeException {}
        private final SocketChannel channel;
        private final UserPrincipal peer;
        private final Runnable stop;
        private final Selector selector;
        private final SelectionKey key;
        private final FrameCodec.Decoder decoder = new FrameCodec.Decoder();
        private final ByteBuffer input = ByteBuffer.allocate(8192);
        private final ArrayBlockingQueue<ByteBuffer> queue = new ArrayBlockingQueue<>(128);
        private final Semaphore slots = new Semaphore(128);
        private final AtomicBoolean closed = new AtomicBoolean();
        private ByteBuffer writing;
        private Connection(SocketChannel channel, UserPrincipal peer, Runnable stop) throws IOException {
            this.channel = channel;
            this.peer = peer;
            this.stop = stop;
            channel.configureBlocking(false);
            selector = Selector.open();
            try { key = channel.register(selector, SelectionKey.OP_READ); }
            catch (IOException | RuntimeException e) { selector.close(); throw e; }
        }
        public UserPrincipal peer() { return peer; }
        public boolean isClosed() { return closed.get(); }
        public void enqueue(Map<String,Object> message) throws IOException {
            FrameCodec.requireDirection(message, FrameCodec.Direction.TO_RUNTIME);
            byte[] bytes = FrameCodec.frame(message);
            if (closed.get()) throw new IOException("closed surface link");
            if (!slots.tryAcquire()) {
                close();
                throw new FrameCodec.ProtocolException("surface control queue exhausted");
            }
            // Capacity includes the partially written frame, not just queued frames.
            if (!queue.offer(ByteBuffer.wrap(bytes))) {
                slots.release();
                close();
                throw new FrameCodec.ProtocolException("surface control queue exhausted");
            }
            selector.wakeup();
        }
        public void poll(Duration timeout, Consumer<Map<String,Object>> receiver) throws IOException {
            if (timeout.isNegative() || timeout.compareTo(Duration.ofSeconds(1)) > 0)
                throw new IllegalArgumentException("poll interval must be between zero and one second");
            if (closed.get()) return;
            try {
                key.interestOps(SelectionKey.OP_READ | (writing != null || !queue.isEmpty() ? SelectionKey.OP_WRITE : 0));
                long millis = timeout.toMillis();
                if (millis == 0) selector.selectNow(); else selector.select(millis);
                if (closed.get()) return;
                var it = selector.selectedKeys().iterator();
                while (it.hasNext()) {
                    SelectionKey selected = it.next();
                    it.remove();
                    if (selected.isReadable()) {
                        input.clear();
                        int n = channel.read(input);
                        if (n < 0) {
                            decoder.endOfInput();
                            close();
                            return;
                        }
                        input.flip();
                        decoder.feed(input, message -> {
                            if (closed.get()) throw new ClosedDispatch();
                            FrameCodec.requireDirection(message, FrameCodec.Direction.TO_SURFACE);
                            receiver.accept(message);
                        });
                    }
                    if (!closed.get() && selected.isWritable()) {
                        if (writing == null) writing = queue.poll();
                        if (writing != null) {
                            channel.write(writing);
                            if (!writing.hasRemaining()) { writing = null; slots.release(); }
                        }
                    }
                }
            } catch (ClosedDispatch e) {
                // A receiver or independent off-switch closed this link mid-read.
                // Do not dispatch any later frame from the coalesced input.
            } catch (IOException | RuntimeException e) {
                close();
                throw e;
            }
        }
        @Override public void close() throws IOException {
            if (!closed.compareAndSet(false, true)) return;
            // Reflex first; no monitor, disk, callback queue or selector wait.
            try { stop.run(); }
            finally {
                try { channel.close(); }
                finally { selector.close(); }
            }
        }
    }
}
