package org.coglet.surface;

import com.fasterxml.jackson.databind.*;
import java.io.*;
import java.lang.reflect.*;
import java.math.BigInteger;
import java.nio.*;
import java.nio.charset.*;
import java.util.*;
import java.util.regex.Pattern;

/** Native RFC 8949 deterministic subset, validated against generated CDDL IR. */
public final class Codec {
    private Codec() {}
    private static final BigInteger LIMIT=BigInteger.ONE.shiftLeft(64);
    private static final BigInteger MIN=BigInteger.ONE.shiftLeft(63).negate();
    private static final ObjectMapper JSON=new ObjectMapper().enable(DeserializationFeature.USE_BIG_INTEGER_FOR_INTS);
    private static final JsonNode SCHEMA;
    private record Binding(String rule, Class<?> type, JsonNode schema, String[] keys) {}
    private static final List<Binding> BINDINGS=new ArrayList<>();
    static {
        try(var in=Codec.class.getResourceAsStream("/schema.json")) {
            if(in==null) throw new IllegalStateException("missing generated schema");
            SCHEMA=JSON.readTree(in);
            Generated.registerAll();
        } catch(IOException e) { throw new ExceptionInInitializerError(e); }
    }
    static void register(String rule,Class<?> type,String schema,String[] keys) {
        try { BINDINGS.add(new Binding(rule,type,JSON.readTree(schema),keys)); }
        catch(IOException e) { throw new ExceptionInInitializerError(e); }
    }
    public static boolean hasRecords(String rule) { return BINDINGS.stream().anyMatch(b->b.rule.equals(rule)); }
    private static IllegalArgumentException bad(String message) { return new IllegalArgumentException(message); }
    private static BigInteger integer(Object v) {
        if(v instanceof BigInteger n) return n;
        if(v instanceof Byte || v instanceof Short || v instanceof Integer || v instanceof Long) return BigInteger.valueOf(((Number)v).longValue());
        throw bad("not an integer");
    }
    private static byte[] utf8(String s) {
        try { var buf=StandardCharsets.UTF_8.newEncoder().onMalformedInput(CodingErrorAction.REPORT).encode(java.nio.CharBuffer.wrap(s)); byte[] out=new byte[buf.remaining()];buf.get(out);return out; }
        catch(CharacterCodingException e) { throw bad("invalid UTF-16 text"); }
    }
    private static void head(ByteArrayOutputStream out,int major,BigInteger n) {
        if(n.signum()<0 || n.compareTo(LIMIT)>=0) throw bad("integer outside uint64");
        int width=n.compareTo(BigInteger.valueOf(24))<0?0:n.bitLength()<=8?1:n.bitLength()<=16?2:n.bitLength()<=32?4:8;
        out.write((major<<5)|(width==0?n.intValue():width==1?24:width==2?25:width==4?26:27));
        for(int i=width-1;i>=0;i--) out.write(n.shiftRight(i*8).intValue()&255);
    }
    public static byte[] encodeProfile(Object value) {
        var out=new ByteArrayOutputStream(); write(out,value,0); return out.toByteArray();
    }
    private static void write(ByteArrayOutputStream out,Object v,int depth) {
        if(depth>64) throw bad("CBOR nesting limit");
        if(v==null) {out.write(246);return;}
        if(v instanceof Boolean b) {out.write(b?245:244);return;}
        if(v instanceof String s) {byte[] raw=utf8(s);head(out,3,BigInteger.valueOf(raw.length));out.writeBytes(raw);return;}
        if(v instanceof List<?> list) {head(out,4,BigInteger.valueOf(list.size()));for(Object x:list) write(out,x,depth+1);return;}
        if(v instanceof Map<?,?> map) {
            record Pair(byte[] key,Object value) {}
            var pairs=new ArrayList<Pair>();
            for(var e:map.entrySet()) {
                if(!(e.getKey() instanceof String s)) throw bad("nontext map key");
                var key=new ByteArrayOutputStream();write(key,s,depth+1);pairs.add(new Pair(key.toByteArray(),e.getValue()));
            }
            pairs.sort((a,b)->Arrays.compareUnsigned(a.key,b.key));head(out,5,BigInteger.valueOf(pairs.size()));
            for(var p:pairs) {out.writeBytes(p.key);write(out,p.value,depth+1);}return;
        }
        BigInteger n=integer(v);
        if(n.compareTo(MIN)<0) throw bad("integer below int64 minimum");
        if(n.signum()<0) {head(out,1,n.negate().subtract(BigInteger.ONE));} else head(out,0,n);
    }
    public static Object decodeProfile(byte[] raw) {
        var reader=new Reader(raw); Object value=reader.read(0);
        if(reader.pos!=raw.length) throw bad("trailing CBOR"); return value;
    }
    private static final class Reader {
        final byte[] raw;int pos;
        Reader(byte[] raw) {this.raw=Objects.requireNonNull(raw);}
        int octet() {if(pos>=raw.length) throw bad("truncated CBOR");return raw[pos++]&255;}
        Object read(int depth) {
            if(depth>64) throw bad("CBOR nesting limit");
            int h=octet(), major=h>>5,ai=h&31;
            if(ai>=28 || major==2 || major==6) throw bad("forbidden CBOR type");
            if(major==7) return switch(ai) {case 20->false;case 21->true;case 22->null;default->throw bad("forbidden simple/float");};
            BigInteger n=BigInteger.valueOf(ai);
            if(ai>=24) {
                int width=1<<(ai-24);byte[] bytes=new byte[width];for(int i=0;i<width;i++) bytes[i]=(byte)octet();n=new BigInteger(1,bytes);
                BigInteger min=width==1?BigInteger.valueOf(24):BigInteger.ONE.shiftLeft(8*(width/2));
                if(n.compareTo(min)<0) throw bad("nonshortest CBOR");
            }
            if(major==0) return n;
            if(major==1) {BigInteger value=n.negate().subtract(BigInteger.ONE);if(value.compareTo(MIN)<0) throw bad("integer below int64 minimum");return value;}
            if(n.compareTo(BigInteger.valueOf(raw.length-pos))>0) throw bad("truncated or huge container");
            int count=n.intValueExact();
            if(major==3) {
                try {String s=StandardCharsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(raw,pos,count)).toString();pos+=count;return s;}
                catch(CharacterCodingException e) {throw bad("invalid UTF-8");}
            }
            if(major==4) {var list=new ArrayList<>();for(int i=0;i<count;i++) list.add(read(depth+1));return list;}
            if(major==5) {
                if(count>(raw.length-pos)/2) throw bad("truncated map");
                var map=new LinkedHashMap<String,Object>();byte[] previous=null;
                for(int i=0;i<count;i++) {
                    int start=pos;Object key=read(depth+1);byte[] encoded=Arrays.copyOfRange(raw,start,pos);
                    if(!(key instanceof String s)) throw bad("nontext map key");
                    if(previous!=null && Arrays.compareUnsigned(previous,encoded)>=0) throw bad("duplicate/unsorted key");
                    previous=encoded;map.put(s,read(depth+1));
                }return map;
            }
            throw bad("forbidden CBOR");
        }
    }
    public static void validate(String rule,Object value) {
        encodeProfile(value);JsonNode node=SCHEMA.get(rule);if(node==null) throw bad("unknown rule "+rule);check(node,value,0);
    }
    private static void check(JsonNode node,Object v,int depth) {
        if(depth>128) throw bad("schema recursion limit");
        String op=node.get("op").asText();boolean good=false;
        switch(op) {
            case "ref" -> {JsonNode target=SCHEMA.get(node.get("name").asText());if(target==null) throw bad("unknown reference");check(target,v,depth+1);return;}
            case "union" -> {for(JsonNode branch:node.get("items")) {try {check(branch,v,depth+1);return;} catch(IllegalArgumentException ignored) {}}throw bad("no union branch matched");}
            case "literal" -> {JsonNode x=node.get("value");good=x.isNull()?v==null:x.isBoolean()?v instanceof Boolean && x.booleanValue()==(Boolean)v:x.isTextual()?v instanceof String && x.textValue().equals(v):v instanceof Number && x.bigIntegerValue().equals(integer(v));}
            case "primitive" -> {good=switch(node.get("name").asText()) {case "tstr"->v instanceof String;case "bool"->v instanceof Boolean;case "null"->v==null;default->false;};}
            case "range" -> {if(v instanceof Number) {BigInteger n=integer(v);good=n.compareTo(node.get("min").bigIntegerValue())>=0&&n.compareTo(node.get("max").bigIntegerValue())<=0;}}
            case "regexp" -> {good=v instanceof String s && Pattern.compile(node.get("pattern").asText()).matcher(s).matches();}
            case "size" -> {if(v instanceof String s) {BigInteger size=BigInteger.valueOf(utf8(s).length);good=size.compareTo(node.get("min").bigIntegerValue())>=0&&size.compareTo(node.get("max").bigIntegerValue())<=0;}}
            case "array" -> {if(v instanceof List<?> xs) {good=xs.size()>=node.get("min").asInt();for(Object x:xs) check(node.get("item"),x,depth+1);}}
            case "map" -> {if(v instanceof Map<?,?> xs) {good=true;for(var e:xs.entrySet()) {if(!(e.getKey() instanceof String)) throw bad("nontext key");check(node.get("item"),e.getValue(),depth+1);}}}
            case "record" -> {if(v instanceof Map<?,?> m) {
                Set<String> keys=new HashSet<>();good=true;
                for(JsonNode f:node.get("fields")) {String k=f.get("name").asText();keys.add(k);if(m.containsKey(k)) check(f.get("type"),m.get(k),depth+1);else if(!f.get("optional").asBoolean()) good=false;}
                if(!keys.containsAll(m.keySet())) good=false;
            }}
            default -> throw bad("unsupported schema op");
        }
        if(!good) throw bad("value does not match "+op);
    }
    public static Object plain(Object v) { return plain(v,0); }
    private static Object plain(Object v,int depth) {
        if(depth>64) throw bad("CBOR nesting limit");
        if(v instanceof Generated.Message m) v=m.toValue();
        if(v instanceof List<?> xs) {var out=new ArrayList<>();for(Object x:xs) out.add(plain(x,depth+1));return out;}
        if(v instanceof Map<?,?> m) {var out=new LinkedHashMap<>();for(var e:m.entrySet()) out.put(e.getKey(),plain(e.getValue(),depth+1));return out;}
        return v;
    }
    public static byte[] encode(String rule,Object value) {
        Object original=value;value=plain(value);validate(rule,value);
        if(original instanceof Generated.Message) {
            for(Binding b:BINDINGS) if(b.type==original.getClass()) {check(b.schema,value,0);break;}
        }
        return encodeProfile(value);
    }
    public static Object decode(String rule,byte[] raw) {Object value=decodeProfile(raw);validate(rule,value);return value;}
    public static Object decodeTyped(String rule,byte[] raw) {Object value=decode(rule,raw);for(Binding b:BINDINGS) if(b.rule.equals(rule)) {try {check(b.schema,value,0);return construct(b,value);} catch(IllegalArgumentException ignored) {}}throw bad("no typed record for rule "+rule);}
    public static <T> T decodeRecord(String rule,byte[] raw,Class<T> type) {
        Object value=decode(rule,raw);
        for(Binding b:BINDINGS) if(b.rule.equals(rule)&&b.type==type) {check(b.schema,value,0);return type.cast(construct(b,value));}
        throw bad("unknown record class");
    }
    private static Object construct(Binding b,Object value) {
        if(!(value instanceof Map<?,?> m)) throw bad("expected record");
        var components=b.type.getRecordComponents();Object[] args=new Object[components.length];Class<?>[] types=new Class<?>[components.length];
        for(int i=0;i<components.length;i++) {types[i]=components[i].getType();Type t=components[i].getGenericType();String key=b.keys[i];
            if(types[i]==Generated.Presence.class) args[i]=m.containsKey(key)?new Generated.Presence<>(convert(((ParameterizedType)t).getActualTypeArguments()[0],m.get(key))):null;
            else args[i]=convert(t,m.get(key));
        }
        try {return b.type.getDeclaredConstructor(types).newInstance(args);} catch(ReflectiveOperationException e) {throw bad("record construction failed: "+e);}
    }
    private static Object convert(Type type,Object v) {
        if(v==null) return null;
        if(type instanceof ParameterizedType p) {
            if(p.getRawType()==List.class) {var out=new ArrayList<>();for(Object x:(List<?>)v) out.add(convert(p.getActualTypeArguments()[0],x));return out;}
            if(p.getRawType()==Map.class) {var out=new LinkedHashMap<>();for(var e:((Map<?,?>)v).entrySet()) out.put(e.getKey(),convert(p.getActualTypeArguments()[1],e.getValue()));return out;}
        }
        if(type instanceof Class<?> c && Generated.Message.class.isAssignableFrom(c)) {
            for(Binding b:BINDINGS) if(c.isAssignableFrom(b.type)) {try {check(b.schema,v,0);return construct(b,v);} catch(IllegalArgumentException ignored) {}}
            throw bad("no record variant matched");
        }
        return v;
    }
}
