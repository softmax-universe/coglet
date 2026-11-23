package org.coglet.surface;

import com.fasterxml.jackson.databind.*;
import java.nio.file.*;
import java.math.BigInteger;
import java.util.*;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class CodecTest {
    static final ObjectMapper JSON = new ObjectMapper().enable(DeserializationFeature.USE_BIG_INTEGER_FOR_INTS);
    static final String MARKER = "semantic-vectors-v1";

    /** Fixture families come from codegen/contracts.json: generated ones run, draft-validated ones are counted skips. */
    @Test void recursiveFixtures() throws Exception {
        int valid=0, invalid=0, wire=0, typed=0, semantic=0, marked=0, draftFiles=0, files=0;
        JsonNode contracts=JSON.readTree(Path.of("../../codegen/contracts.json").toFile());
        assertTrue(contracts.isObject() && contracts.size()>0);
        Path root=Path.of("../../spec/fixtures");
        Set<String> families=new TreeSet<>();
        try (var dirs=Files.list(root)) { for(Path d:dirs.toList()) { if(Files.isDirectory(d)) families.add(d.getFileName().toString()); else assertFalse(d.toString().endsWith(".json"),"unclassified root fixture "+d); } }
        Set<String> declared=new TreeSet<>(); contracts.fieldNames().forEachRemaining(declared::add);
        assertEquals(declared,families,"unclassified or stale fixture families");
        for(String family:families) {
            String mode=contracts.get(family).asText();
            assertTrue(mode.equals("generated")||mode.equals("draft-validated"),"unknown contract mode "+family+": "+mode);
            List<Path> jsons;
            try (var paths=Files.walk(root.resolve(family))) { jsons=paths.filter(x->x.toString().endsWith(".json")).sorted().toList(); }
            assertFalse(jsons.isEmpty(),"empty fixture family "+family);
            if(mode.equals("draft-validated")) { draftFiles+=jsons.size(); continue; } // validated by their own drafts; not in the generated schema
            for (Path p : jsons) {
                files++;
                JsonNode cases=JSON.readTree(p.toFile()); // malformed JSON always fails, marked or not
                Path marker=p.resolveSibling(p.getFileName()+".format");
                if(Files.exists(marker)) {
                    String text=Files.readString(marker);
                    assertTrue(text.equals(MARKER)||text.equals(MARKER+"\n"),"unknown fixture format in "+marker+": "+text);
                    marked++; continue;
                }
                assertTrue(cases.isArray() && cases.size()>0, "fixture file must be a nonempty array or explicitly marked: "+p);
                Set<String> seen=new HashSet<>();
                for (JsonNode c : cases) {
                    assertTrue(c.isObject() && c.hasNonNull("name") && c.get("name").isTextual() && seen.add(c.get("name").asText()), "malformed or duplicate fixture: "+p);
                    String name=p+":"+c.get("name").asText();
                    try {
                    if (!c.has("valid")) {
                        assertTrue(c.hasNonNull("cbor_hex") && c.get("cbor_hex").isTextual() && !c.has("type") && !c.has("semantic"), name);
                        byte[] raw=HexFormat.of().parseHex(c.get("cbor_hex").asText());
                        assertThrows(IllegalArgumentException.class, () -> Codec.decodeProfile(raw), name); wire++;
                    } else {
                        assertTrue(c.get("valid").isBoolean() && c.hasNonNull("type") && c.get("type").isTextual() && c.has("value"), name);
                        assertTrue(!c.has("semantic") || c.get("semantic").isBoolean(), name);
                        String rule=c.get("type").asText();
                        Object value=JSON.convertValue(c.get("value"), Object.class);
                        boolean isSemantic=c.has("semantic") && c.get("semantic").asBoolean();
                        if(c.get("valid").asBoolean()) {
                            assertTrue(c.hasNonNull("cbor_hex") && c.get("cbor_hex").isTextual(), name);
                            byte[] raw=HexFormat.of().parseHex(c.get("cbor_hex").asText());
                            assertArrayEquals(raw, Codec.encode(rule, value), name);
                            assertArrayEquals(raw, Codec.encode(rule, Codec.decode(rule,raw)), name);
                            if(Codec.hasRecords(rule)) {
                                var message=(Generated.Message)Codec.decodeTyped(rule,raw);
                                assertArrayEquals(raw,message.toCbor(),name); typed++;
                            }
                            valid++;
                        } else if(isSemantic) {
                            // Protocol-semantic negative: the SHAPE must be valid (unknown types and
                            // malformed shapes still fail); the semantic outcome belongs to the protocol suite.
                            Codec.validate(rule,value);
                            if(c.hasNonNull("cbor_hex")) Codec.validate(rule,Codec.decodeProfile(HexFormat.of().parseHex(c.get("cbor_hex").asText())));
                            semantic++;
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
                    } catch (IllegalArgumentException e) { throw new AssertionError("fixture case "+name+" failed: "+e, e); }
                }
            }
        }
        assertTrue(valid>=33); assertTrue(invalid>=43); assertTrue(wire>=17); assertTrue(typed>=24);
        System.out.printf("Java fixtures: %d files, %d golden, %d shape-invalid, %d wire-invalid, %d semantic-shape (counted separately), %d typed; skipped %d semantic-vectors-v1 files and %d draft-validated files%n",files,valid,invalid,wire,semantic,typed,marked,draftFiles);
    }

    // ---- additive IR v2 node semantics, exercised on explicit rule dictionaries ----
    static JsonNode rules(String json) { try { return JSON.readTree(json); } catch(Exception e) { throw new AssertionError(e); } }
    static void ok(JsonNode r,String rule,Object v) { Codec.validateWith(r,rule,v); }
    static void no(JsonNode r,String rule,Object v) { assertThrows(IllegalArgumentException.class,()->Codec.validateWith(r,rule,v),String.valueOf(v)); }
    static BigInteger n(long x) { return BigInteger.valueOf(x); }
    static final String TSTR="{\"op\":\"primitive\",\"name\":\"tstr\"}";
    static String range(int lo,int hi) { return "{\"op\":\"range\",\"min\":"+lo+",\"max\":"+hi+"}"; }

    @Test void arrayBounds() {
        var r=rules("{\"a\":{\"op\":\"array\",\"item\":"+range(0,9)+",\"min\":1,\"max\":2},\"u\":{\"op\":\"array\",\"item\":"+range(0,9)+",\"min\":0}}");
        no(r,"a",List.of()); ok(r,"a",List.of(n(1))); ok(r,"a",List.of(n(1),n(2))); no(r,"a",List.of(n(1),n(2),n(3)));
        no(r,"a",List.of(n(10))); no(r,"a","x");
        ok(r,"u",List.of()); ok(r,"u",Collections.nCopies(500,n(1))); no(r,"u",List.of(n(10)));
    }
    @Test void mapBoundsAndKey() {
        var r=rules("{\"m\":{\"op\":\"map\",\"item\":"+range(0,9)+",\"min\":1,\"max\":2,\"key\":{\"op\":\"regexp\",\"pattern\":\"[a-z]+\"}},"
            +"\"free\":{\"op\":\"map\",\"item\":"+range(0,9)+"}}");
        no(r,"m",Map.of()); ok(r,"m",Map.of("a",n(1))); ok(r,"m",Map.of("a",n(1),"bb",n(2)));
        no(r,"m",Map.of("a",n(1),"b",n(2),"c",n(3)));
        no(r,"m",Map.of("A",n(1)));            // key constraint
        no(r,"m",Map.of("a",n(10)));           // value constraint
        no(r,"m",List.of());
        ok(r,"free",Map.of()); ok(r,"free",Map.of("Anything-1",n(1)));
        assertThrows(IllegalArgumentException.class,()->Codec.validateWith(r,"free",Map.of(1,n(1))));
    }
    @Test void recordRestBoundsAndKey() {
        var r=rules("{\"closed\":{\"op\":\"record\",\"fields\":[{\"name\":\"id\",\"optional\":false,\"type\":"+TSTR+"}]},"
            +"\"open\":{\"op\":\"record\",\"fields\":[{\"name\":\"id\",\"optional\":false,\"type\":"+TSTR+"}],\"rest\":{\"item\":"+range(0,9)+",\"min\":1,\"max\":2,\"key\":{\"op\":\"regexp\",\"pattern\":\"x-[a-z]+\"}}},"
            +"\"anykey\":{\"op\":\"record\",\"fields\":[],\"rest\":{\"item\":"+range(0,9)+",\"min\":0}}}");
        ok(r,"closed",Map.of("id","a")); no(r,"closed",Map.of("id","a","x-y",n(1)));
        no(r,"open",Map.of("id","a"));                                         // rest.min
        ok(r,"open",Map.of("id","a","x-a",n(1))); ok(r,"open",Map.of("id","a","x-a",n(1),"x-b",n(2)));
        no(r,"open",Map.of("id","a","x-a",n(1),"x-b",n(2),"x-c",n(3)));      // rest.max
        no(r,"open",Map.of("id","a","y",n(1)));                                // rest.key
        no(r,"open",Map.of("id","a","x-a",n(10)));                             // rest.item
        no(r,"open",Map.of("x-a",n(1)));                                       // required field; claimed key not counted as rest
        no(r,"open",Map.of("id",n(1),"x-a",n(1)));
        ok(r,"anykey",Map.of()); ok(r,"anykey",Map.of("whatever",n(9))); no(r,"anykey",Map.of("whatever",n(10)));
    }
    @Test void sizeIsUtf8BytesWithOptionalMax() {
        var r=rules("{\"s\":{\"op\":\"size\",\"min\":1,\"max\":2},\"u\":{\"op\":\"size\",\"min\":2}}");
        no(r,"s",""); ok(r,"s","ab"); ok(r,"s","\u00e9"); no(r,"s","\u00e9a"); no(r,"s",n(1));
        no(r,"u","a"); ok(r,"u","\u00e9"); ok(r,"u","x".repeat(10000));
    }
    @Test void intersectionAndEmptyType() {
        var r=rules("{\"i\":{\"op\":\"intersection\",\"items\":[{\"op\":\"regexp\",\"pattern\":\"[a-z]+\"},{\"op\":\"size\",\"min\":2,\"max\":3}]},"
            +"\"nilfalse\":{\"op\":\"intersection\",\"items\":[{\"op\":\"literal\",\"value\":null},{\"op\":\"literal\",\"value\":false}]}}");
        ok(r,"i","ab"); ok(r,"i","abc"); no(r,"i","a"); no(r,"i","abcd"); no(r,"i","AB"); no(r,"i",n(1));
        // The empty type: neither null nor false may validate.
        no(r,"nilfalse",null); no(r,"nilfalse",false); no(r,"nilfalse",true); no(r,"nilfalse","x");
    }
    @Test void defaultIsMetadataOnly() {
        var r=rules("{\"d\":{\"op\":\"default\",\"item\":"+range(0,9)+",\"value\":5},"
            +"\"rec\":{\"op\":\"record\",\"fields\":[{\"name\":\"n\",\"optional\":true,\"type\":{\"op\":\"default\",\"item\":"+range(0,9)+",\"value\":5}}]}}");
        ok(r,"d",n(0)); ok(r,"d",n(9)); no(r,"d",n(10)); no(r,"d","5");
        ok(r,"rec",Map.of());                                  // missing optional stays missing; no insertion
        ok(r,"rec",Map.of("n",n(1))); no(r,"rec",Map.of("n",n(10)));
        var present=new HashMap<String,Object>(); present.put("n",null); no(r,"rec",present);
    }
    @Test void tupleBacktrackingAndOccurrences() {
        var r=rules("{\"t\":{\"op\":\"tuple\",\"items\":[{\"type\":"+TSTR+",\"min\":0,\"max\":2},{\"type\":"+TSTR+",\"min\":1,\"max\":1}]},"
            +"\"b\":{\"op\":\"tuple\",\"items\":[{\"type\":"+range(0,9)+",\"min\":1,\"max\":3},{\"type\":{\"op\":\"literal\",\"value\":5},\"min\":1,\"max\":1}]},"
            +"\"mix\":{\"op\":\"tuple\",\"items\":[{\"type\":{\"op\":\"primitive\",\"name\":\"bool\"},\"min\":1,\"max\":1},{\"type\":"+TSTR+",\"min\":0}]},"
            +"\"empty\":{\"op\":\"tuple\",\"items\":[]}}");
        no(r,"t",List.of()); ok(r,"t",List.of("a")); ok(r,"t",List.of("a","b")); ok(r,"t",List.of("a","b","c"));
        no(r,"t",List.of("a","b","c","d")); no(r,"t",List.of("a",n(1))); no(r,"t","a");
        ok(r,"b",List.of(n(5),n(5)));          // greedy first entry would starve the literal; must backtrack
        ok(r,"b",List.of(n(1),n(2),n(3),n(5))); no(r,"b",List.of(n(1),n(2),n(3),n(4),n(5))); no(r,"b",List.of(n(5)));
        ok(r,"mix",List.of(true)); ok(r,"mix",List.of(true,"a","b","c")); no(r,"mix",List.of("a")); no(r,"mix",List.of(true,n(1)));
        ok(r,"empty",List.of()); no(r,"empty",List.of(n(1)));
    }
    @Test void qualifiedRuleKeysAndAliases() {
        var r=rules("{\"common::id\":"+range(1,9)+",\"id\":"+range(1,9)+",\"p::wire\":{\"op\":\"record\",\"fields\":[{\"name\":\"id\",\"optional\":false,\"type\":{\"op\":\"ref\",\"name\":\"common::id\"}}]}}");
        ok(r,"common::id",n(1)); ok(r,"id",n(9)); no(r,"common::id",n(0));
        ok(r,"p::wire",Map.of("id",n(3))); no(r,"p::wire",Map.of("id",n(0))); no(r,"wire",Map.of("id",n(3)));
    }
    @Test void boundsBeyondInt64DoNotWrap() {
        var r=rules("{\"s\":{\"op\":\"size\",\"min\":1,\"max\":18446744073709551615},\"a\":{\"op\":\"array\",\"item\":"+TSTR+",\"min\":0,\"max\":18446744073709551615}}");
        ok(r,"s","a"); no(r,"s",""); ok(r,"a",List.of("x"));
    }
    @Test void unknownOpAndRefFailClosed() {
        var r=rules("{\"x\":{\"op\":\"bytes\"},\"y\":{\"op\":\"ref\",\"name\":\"missing\"}}");
        no(r,"x",n(1)); no(r,"y",n(1)); no(r,"absent",n(1));
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
