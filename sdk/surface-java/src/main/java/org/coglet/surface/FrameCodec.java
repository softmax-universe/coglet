package org.coglet.surface;

import java.nio.ByteBuffer;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;

/** Frozen surface/0.1 framing. This is not operation admission or authentication. */
public final class FrameCodec {
    public static final int MAX_FRAME = 1_048_576;
    public static final String WIRE_RULE = "surface::surface-message";
    private FrameCodec() {}

    public enum Direction { TO_RUNTIME, TO_SURFACE }
    public static final class ProtocolException extends IllegalArgumentException {
        public ProtocolException(String message) { super(message); }
        public ProtocolException(String message, Throwable cause) { super(message, cause); }
    }

    /** Validate the generated closed wire schema, then add the network-order length. */
    public static byte[] frame(Map<String,Object> message) {
        Codec.validate(WIRE_RULE, message);
        byte[] raw = Codec.encodeProfile(message);
        if (raw.length == 0 || raw.length > MAX_FRAME) throw new ProtocolException("frame size");
        return ByteBuffer.allocate(4 + raw.length).putInt(raw.length).put(raw).array();
    }

    /** The supervisor/session must additionally check negotiation and selected operation rules. */
    public static void requireDirection(Map<String,Object> message, Direction direction) {
        Set<String> allowed = direction == Direction.TO_RUNTIME
                ? Set.of("hello", "capabilities", "grants", "event", "read_reply", "accept", "health", "kill", "error")
                : Set.of("hello_reply", "read", "act", "health", "error");
        if (!allowed.contains(message.get("type"))) throw new ProtocolException("wrong message direction");
    }

    /** One control-thread decoder; stores at most one bounded payload. Fatal is sticky. */
    public static final class Decoder {
        private final ByteBuffer header = ByteBuffer.allocate(4);
        private ByteBuffer payload;
        private boolean failed;

        public void feed(ByteBuffer input, Consumer<Map<String,Object>> receiver) {
            if (failed) throw new ProtocolException("decoder is closed");
            try {
                while (input.hasRemaining()) {
                    if (payload == null) {
                        copy(input, header);
                        if (header.hasRemaining()) continue;
                        int length = header.flip().getInt();
                        header.clear();
                        // Check the unsigned prefix BEFORE allocating its payload.
                        if (length <= 0 || length > MAX_FRAME) throw new ProtocolException("frame size");
                        payload = ByteBuffer.allocate(length);
                    }
                    copy(input, payload);
                    if (payload.hasRemaining()) continue;
                    Object value;
                    try {
                        value = Codec.decodeProfile(payload.array());
                        Codec.validate(WIRE_RULE, value);
                    } catch (IllegalArgumentException e) {
                        throw new ProtocolException("invalid surface frame", e);
                    }
                    @SuppressWarnings("unchecked")
                    Map<String,Object> message = (Map<String,Object>) value;
                    payload = null;
                    receiver.accept(message);
                }
            } catch (RuntimeException e) {
                failed = true;
                payload = null;
                throw e;
            }
        }

        public void endOfInput() {
            boolean partial = payload != null || header.position() != 0;
            if (failed || partial) {
                failed = true;
                payload = null;
                throw new ProtocolException("EOF inside frame or after fatal error");
            }
        }
        private static void copy(ByteBuffer input, ByteBuffer target) {
            int count = Math.min(input.remaining(), target.remaining());
            ByteBuffer slice = input.slice();
            slice.limit(count);
            target.put(slice);
            input.position(input.position() + count);
        }
    }
}
