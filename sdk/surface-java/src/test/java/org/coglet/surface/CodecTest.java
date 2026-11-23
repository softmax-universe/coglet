package org.coglet.surface;

import com.fasterxml.jackson.databind.*;
import java.nio.file.*;
import java.math.BigInteger;
import java.util.*;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class CodecTest {
    static final ObjectMapper JSON = new ObjectMapper().enable(DeserializationFeature.USE_BIG_INTEGER_FOR_INTS);
    @Test void recursiveFixtures() throws Exception {
        int valid=0, invalid=0, wire=0, typed=0;
        try (var paths = Files.walk(Path.of("../../spec/fixtures"))) {
            for (Path p : paths.filter(x -> x.toString().endsWith(".json")).sorted().toList()) {
                JsonNode cases=JSON.readTree(p.toFile());
                assertTrue(cases.isArray(), "fixture file must be an array: "+p);
                for (JsonNode c : cases) {
                    assertTrue(c.isObject() && c.hasNonNull("name") && c.get("name").isTextual(), "malformed fixture: "+p);
                    String name=p+":"+c.get("name").asText();
                    if (!c.has("valid")) {
                        assertTrue(c.hasNonNull("cbor_hex") && c.get("cbor_hex").isTextual(), name);
                        byte[] raw=HexFormat.of().parseHex(c.get("cbor_hex").asText());
                        assertThrows(IllegalArgumentException.class, () -> Codec.decodeProfile(raw), name); wire++;
                    } else {
                        String rule=c.get("type").asText();
                        assertTrue(c.get("valid").isBoolean() && c.hasNonNull("type") && c.get("type").isTextual() && c.has("value"), name);
                        Object value=JSON.convertValue(c.get("value"), Object.class);
                        if(c.get("valid").asBoolean()) {
                            byte[] raw=HexFormat.of().parseHex(c.get("cbor_hex").asText());
                            assertArrayEquals(raw, Codec.encode(rule, value), name);
                            assertArrayEquals(raw, Codec.encode(rule, Codec.decode(rule,raw)), name);
                            if(Codec.hasRecords(rule)) {
                                var message=(Generated.Message)Codec.decodeTyped(rule,raw);
                                assertArrayEquals(raw,message.toCbor(),name); typed++;
                            }
                            valid++;
                        } else {
                            assertThrows(IllegalArgumentException.class, () -> Codec.encode(rule,value),name);
                            // Values outside the native profile (e.g. 2^64) have no legal
                            // wire representation; encode rejection is the applicable check.
                            try {
                                byte[] raw=Codec.encodeProfile(value);
                                assertThrows(IllegalArgumentException.class, () -> Codec.decode(rule,raw),name);
                            } catch (IllegalArgumentException outsideProfile) { }
                            invalid++;
                        }
                    }
                }
            }
        }
        assertTrue(valid>=33); assertTrue(invalid>=43); assertTrue(wire>=17); assertTrue(typed>=24);
        System.out.printf("Java fixtures: %d golden, %d semantic-invalid, %d wire-invalid; %d typed%n",valid,invalid,wire,typed);
    }
    @Test void typedBranchAndSchemaDepth() {
        var accepted=new Generated.ActionAccept1("0".repeat(32),"1".repeat(32),true);
        byte[] raw=accepted.toCbor();
        assertInstanceOf(Generated.ActionAccept1.class,Generated.ActionAccept.fromCbor(raw));
        assertThrows(IllegalArgumentException.class,()->Generated.ActionAccept2.fromCbor(raw));
        var incorrect=new Generated.ActionAccept1("0".repeat(32),"1".repeat(32),false);
        // The general union accepts false only with an error, so branch mismatch fails.
        assertThrows(IllegalArgumentException.class,incorrect::toCbor);
        Object value=BigInteger.ZERO;
        for(int i=0;i<40;i++) value=List.of(value);
        Codec.validate("profile-value",value);
        for(int i=0;i<8;i++) value=List.of(value);
        final Object tooDeep=value;
        assertNotNull(Codec.encodeProfile(tooDeep));
        assertThrows(IllegalArgumentException.class,()->Codec.validate("profile-value",tooDeep));
    }
    @Test void profileBoundaries() {
        BigInteger limit=BigInteger.ONE.shiftLeft(64);
        BigInteger min=BigInteger.ONE.shiftLeft(63).negate();
        for(BigInteger n: List.of(limit.subtract(BigInteger.ONE),min))
            assertEquals(n,Codec.decodeProfile(Codec.encodeProfile(n)));
        for(Object x: List.of(limit,min.subtract(BigInteger.ONE),1.0,new byte[]{1},Map.of(1,"a"),"\ud800"))
            assertThrows(IllegalArgumentException.class,()->Codec.encodeProfile(x));
        Object x=BigInteger.ZERO;
        for(int i=0;i<64;i++) x=List.of(x);
        assertNotNull(Codec.decodeProfile(Codec.encodeProfile(x)));
        final Object deep=List.of(x);
        assertThrows(IllegalArgumentException.class,()->Codec.encodeProfile(deep));
        Object deeper=BigInteger.ZERO;
        for(int i=0;i<10000;i++) deeper=List.of(deeper);
        final Object untrusted=deeper;
        assertThrows(IllegalArgumentException.class,()->Codec.encode("profile-value",untrusted));
        assertThrows(IllegalArgumentException.class,()->Codec.decodeProfile(HexFormat.of().parseHex("81".repeat(65)+"00")));
        for(String h:List.of("3b8000000000000000","3bffffffffffffffff","9bffffffffffffffff","7bffffffffffffffff","a2616200616100","f814","d9d9f700"))
            assertThrows(IllegalArgumentException.class,()->Codec.decodeProfile(HexFormat.of().parseHex(h)));
    }
}
