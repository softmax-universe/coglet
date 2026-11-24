package org.coglet.surface;

import static org.junit.jupiter.api.Assertions.*;
import java.nio.ByteBuffer;
import java.util.*;
import org.junit.jupiter.api.Test;

class FrameCodecTest {
    static final String ID = "1".repeat(32);
    static Map<String,Object> health() {
        return Map.of("type", "health", "body", Map.of("state", "ready", "clock", 0,
                "pending", 0, "correlation_id", ID));
    }
    @Test void fragmentedHeaderPayloadAndCoalescedFrames() {
        byte[] framed = FrameCodec.frame(health());
        var decoder = new FrameCodec.Decoder();
        var output = new ArrayList<Map<String,Object>>();
        for (byte b : framed) decoder.feed(ByteBuffer.wrap(new byte[]{b}), output::add);
        assertEquals(1, output.size());
        assertArrayEquals(Codec.encodeProfile(health()), Codec.encodeProfile(output.getFirst()));
        decoder.endOfInput();
        output.clear();
        var both = ByteBuffer.allocate(framed.length * 2).put(framed).put(framed).flip();
        decoder.feed(both, output::add);
        assertEquals(2, output.size());
    }
    @Test void invalidLengthIsFatalBeforePayloadAllocation() {
        for (int n : new int[]{0, -1, FrameCodec.MAX_FRAME + 1}) {
            var decoder = new FrameCodec.Decoder();
            assertThrows(FrameCodec.ProtocolException.class,
                    () -> decoder.feed(ByteBuffer.allocate(4).putInt(n).flip(), x -> fail()));
            assertThrows(FrameCodec.ProtocolException.class,
                    () -> decoder.feed(ByteBuffer.wrap(FrameCodec.frame(health())), x -> fail()));
        }
    }
    @Test void eofMidHeaderOrPayloadIsFatal() {
        byte[] framed = FrameCodec.frame(health());
        for (int n : new int[]{1, 3, 4, framed.length - 1}) {
            var decoder = new FrameCodec.Decoder();
            decoder.feed(ByteBuffer.wrap(Arrays.copyOf(framed, n)), x -> fail());
            assertThrows(FrameCodec.ProtocolException.class, decoder::endOfInput);
        }
    }
    @Test void invalidCborOrSchemaIsFatal() {
        for (byte[] payload : List.of(new byte[]{(byte)0xf9, 0, 0},
                new byte[]{(byte)0xa0}, Codec.encodeProfile(Map.of("type", "bogus", "body", Map.of())),
                Arrays.copyOf(Codec.encodeProfile(health()), Codec.encodeProfile(health()).length + 1))) {
            var decoder = new FrameCodec.Decoder();
            var buffer = ByteBuffer.allocate(payload.length + 4).putInt(payload.length).put(payload).flip();
            assertThrows(FrameCodec.ProtocolException.class, () -> decoder.feed(buffer, x -> fail()));
        }
    }
    @Test void unknownFieldsAndWrongDirectionAreRefused() {
        var value = new HashMap<>(health());
        value.put("surprise", true);
        assertThrows(IllegalArgumentException.class, () -> FrameCodec.frame(value));
        assertDoesNotThrow(() -> FrameCodec.requireDirection(health(), FrameCodec.Direction.TO_SURFACE));
        assertDoesNotThrow(() -> FrameCodec.requireDirection(health(), FrameCodec.Direction.TO_RUNTIME));
        var hello = Map.<String,Object>of("type", "hello", "body", Map.of("surface", "surface:" + "a".repeat(64),
                "offer", Map.of("versions", List.of("0.1")), "clock", 0, "correlation_id", ID));
        FrameCodec.frame(hello);
        assertThrows(FrameCodec.ProtocolException.class,
                () -> FrameCodec.requireDirection(hello, FrameCodec.Direction.TO_SURFACE));
        assertDoesNotThrow(() -> FrameCodec.requireDirection(hello, FrameCodec.Direction.TO_RUNTIME));
    }
    @Test void consumerFailurePoisonsDecoderAndStopsCoalescedDelivery() {
        var decoder = new FrameCodec.Decoder();
        byte[] framed = FrameCodec.frame(health());
        var both = ByteBuffer.allocate(framed.length * 2).put(framed).put(framed).flip();
        assertThrows(IllegalStateException.class, () -> decoder.feed(both, x -> { throw new IllegalStateException(); }));
        assertThrows(FrameCodec.ProtocolException.class,
                () -> decoder.feed(ByteBuffer.wrap(framed), x -> fail()));
    }
}
