package org.coglet.surface;

import static org.junit.jupiter.api.Assertions.*;
import java.net.*;
import java.nio.*;
import java.nio.channels.*;
import java.nio.file.*;
import java.nio.file.attribute.*;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.*;

@Timeout(10)
class UnixSurfaceListenerTest {
    Path dir;
    @BeforeEach void privateDirectory() throws Exception {
        dir = Files.createTempDirectory(Path.of(System.getenv().getOrDefault("TMPDIR", System.getProperty("java.io.tmpdir"))), "cg-java-");
        Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("rwx------"));
    }
    @AfterEach void cleanup() throws Exception {
        try (var files = Files.list(dir)) { for (Path p : files.toList()) Files.deleteIfExists(p); }
        Files.delete(dir);
    }
    @Test void actualUnixCredentialsPrivatePermissionsAndFragmentedWire() throws Exception {
        var stopped = new AtomicInteger();
        Path path = dir.resolve("s");
        var owner = Files.getOwner(dir);
        try (var listener = new UnixSurfaceListener(path, owner, stopped::incrementAndGet);
             var client = SocketChannel.open(UnixDomainSocketAddress.of(path));
             var connection = listener.accept()) {
            assertEquals(PosixFilePermissions.fromString("rw-------"), Files.getPosixFilePermissions(path));
            assertEquals(owner, connection.peer());
            byte[] bytes = FrameCodec.frame(FrameCodecTest.health());
            var messages = new ArrayList<Map<String,Object>>();
            for (byte b : bytes) {
                client.write(ByteBuffer.wrap(new byte[]{b}));
                connection.poll(Duration.ofMillis(20), messages::add);
            }
            assertEquals(1, messages.size());
            connection.enqueue(FrameCodecTest.health());
            connection.poll(Duration.ofMillis(20), messages::add);
            var output = ByteBuffer.allocate(bytes.length);
            while (output.hasRemaining()) assertTrue(client.read(output) > 0);
            assertArrayEquals(bytes, output.array());
            client.close();
            connection.poll(Duration.ofMillis(20), messages::add);
            assertTrue(connection.isClosed());
            assertEquals(1, stopped.get());
        }
        assertFalse(Files.exists(path));
        assertEquals(1, stopped.get());
    }
    @Test void wrongPeerNeverProducesCapabilitiesOrDeviceData() throws Exception {
        var wrong = new UserPrincipal() { public String getName() { return "never-the-peer"; } };
        var stopped = new AtomicInteger();
        try (var listener = new UnixSurfaceListener(dir.resolve("s"), wrong, stopped::incrementAndGet);
             var client = SocketChannel.open(UnixDomainSocketAddress.of(dir.resolve("s")))) {
            assertThrows(SecurityException.class, listener::accept);
            assertEquals(-1, client.read(ByteBuffer.allocate(1)));
            assertEquals(1, stopped.get());
        }
    }
    @Test void wrongDirectionAndPartialEofStopNativeOwner() throws Exception {
        for (boolean wrongDirection : List.of(false, true)) {
            var stopped = new AtomicInteger();
            try (var listener = new UnixSurfaceListener(dir.resolve("s"), Files.getOwner(dir), stopped::incrementAndGet);
                 var client = SocketChannel.open(UnixDomainSocketAddress.of(dir.resolve("s")));
                 var connection = listener.accept()) {
                if (wrongDirection) {
                    var hello = Map.<String,Object>of("type", "hello", "body", Map.of("surface", "surface:" + "a".repeat(64),
                            "offer", Map.of("versions", List.of("0.1")), "clock", 0, "correlation_id", FrameCodecTest.ID));
                    client.write(ByteBuffer.wrap(FrameCodec.frame(hello)));
                } else {
                    client.write(ByteBuffer.wrap(new byte[]{0, 0}));
                    client.shutdownOutput();
                }
                assertThrows(FrameCodec.ProtocolException.class, () -> {
                    for (int i = 0; i < 4; i++) connection.poll(Duration.ofMillis(20), x -> fail());
                });
                assertEquals(1, stopped.get());
                assertTrue(connection.isClosed());
            }
        }
    }
    @Test void boundedQueueFailsClosedAndCloseDoesNotWaitOnPoll() throws Exception {
        var stopped = new AtomicInteger();
        try (var listener = new UnixSurfaceListener(dir.resolve("s"), Files.getOwner(dir), stopped::incrementAndGet);
             var client = SocketChannel.open(UnixDomainSocketAddress.of(dir.resolve("s")));
             var connection = listener.accept()) {
            for (int i = 0; i < 128; i++) connection.enqueue(FrameCodecTest.health());
            assertThrows(FrameCodec.ProtocolException.class, () -> connection.enqueue(FrameCodecTest.health()));
            assertTrue(connection.isClosed());
            assertEquals(1, stopped.get());
        }
    }
    @Test void nonprivateDirectorySymlinksAndExistingSocketRefused() throws Exception {
        Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("rwxr-x---"));
        assertThrows(SecurityException.class,
                () -> new UnixSurfaceListener(dir.resolve("s"), Files.getOwner(dir), () -> {}));
        Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("rwx------"));
        Path existing = dir.resolve("s");
        Files.writeString(existing, "do not unlink");
        assertThrows(FileAlreadyExistsException.class,
                () -> new UnixSurfaceListener(existing, Files.getOwner(dir), () -> {}));
        assertEquals("do not unlink", Files.readString(existing));
        Files.createSymbolicLink(dir.resolve("link"), dir);
        assertThrows(SecurityException.class,
                () -> new UnixSurfaceListener(dir.resolve("link/s2"), Files.getOwner(dir), () -> {}));
    }
    @Test void closeOnFirstCoalescedFrameStopsFurtherDelivery() throws Exception {
        var stopped = new AtomicInteger();
        try (var listener = new UnixSurfaceListener(dir.resolve("s"), Files.getOwner(dir), stopped::incrementAndGet);
             var client = SocketChannel.open(UnixDomainSocketAddress.of(dir.resolve("s")));
             var connection = listener.accept()) {
            byte[] framed = FrameCodec.frame(FrameCodecTest.health());
            client.write(ByteBuffer.allocate(2 * framed.length).put(framed).put(framed).flip());
            var calls = new AtomicInteger();
            connection.poll(Duration.ofMillis(20), x -> {
                calls.incrementAndGet();
                try { connection.close(); } catch (java.io.IOException e) { throw new RuntimeException(e); }
            });
            assertEquals(1, calls.get());
            assertEquals(1, stopped.get());
        }
    }
    @Test void writableAncestorModeNeedsStickyProtectionUnlessBelowPrivatePrefix() throws Exception {
        assertThrows(SecurityException.class, () -> UnixSurfaceListener.requireNonreplaceableMode(0777));
        assertThrows(SecurityException.class, () -> UnixSurfaceListener.requireNonreplaceableMode(0775));
        assertDoesNotThrow(() -> UnixSurfaceListener.requireNonreplaceableMode(01777));
        assertDoesNotThrow(() -> UnixSurfaceListener.requireNonreplaceableMode(0755));
        // This test's directory lies below the trusted private scratch prefix:
        // another UID cannot traverse that prefix to replace this component.
        Path interior = Files.createDirectory(dir.resolve("interior"));
        Path privateChild = Files.createDirectory(interior.resolve("private"));
        Files.setPosixFilePermissions(privateChild, PosixFilePermissions.fromString("rwx------"));
        Files.setPosixFilePermissions(interior, PosixFilePermissions.fromString("rwxrwxrwx"));
        try (var listener = new UnixSurfaceListener(privateChild.resolve("s"), Files.getOwner(dir), () -> {})) {
            assertTrue(Files.exists(privateChild.resolve("s")));
        } finally {
            Files.delete(privateChild);
            Files.delete(interior);
        }
    }
    @Test void closeDoesNotDeleteReplacedPath() throws Exception {
        Path socket = dir.resolve("s");
        var listener = new UnixSurfaceListener(socket, Files.getOwner(dir), () -> {});
        Files.delete(socket);
        Files.writeString(socket, "replacement");
        listener.close();
        assertEquals("replacement", Files.readString(socket));
    }
}
