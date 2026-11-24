package org.coglet.surface;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.databind.*;
import java.math.BigInteger;
import java.security.*;
import java.util.*;

/** Immutable, explicitly pinned local supervisor configuration. Never fetches schemas.
 * This parses extension descriptors only; it neither registers operations nor grants authority.
 */
public final class TrustedExtensionCatalog {
    private static final ObjectMapper JSON = new ObjectMapper()
            .enable(JsonParser.Feature.STRICT_DUPLICATE_DETECTION)
            .enable(DeserializationFeature.USE_BIG_INTEGER_FOR_INTS)
            .enable(DeserializationFeature.FAIL_ON_TRAILING_TOKENS);
    public record Operation(String entry, String direction, String input, String output, Set<String> resources) {
        public Operation { resources = Set.copyOf(resources); }
    }
    private final String pin;
    private final JsonNode artifact;
    private final Map<String,Operation> operations;
    private TrustedExtensionCatalog(String pin, JsonNode artifact, Map<String,Operation> operations) {
        this.pin = pin; this.artifact = artifact; this.operations = Map.copyOf(operations);
    }
    private static IllegalArgumentException bad(String why) { return new IllegalArgumentException(why); }
    public static TrustedExtensionCatalog fromPinnedBytes(byte[] raw, String pin) {
        if (raw == null || raw.length > 1_048_576 || pin == null || !pin.matches("[0-9a-f]{64}"))
            throw bad("trusted extension catalog pin absent/invalid or byte limit");
        byte[] copy = raw.clone();
        try {
            String hash = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(copy));
            if (!hash.equals(pin)) throw bad("trusted extension catalog pin mismatch");
            String json = java.nio.charset.StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
                    .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT)
                    .decode(java.nio.ByteBuffer.wrap(copy)).toString();
            if (json.startsWith("\ufeff")) throw bad("catalog must be UTF-8 without BOM");
            JsonNode artifact = JSON.readTree(json);
            validateDocument(artifact);
            var operations = new LinkedHashMap<String,Operation>();
            artifact.get("operations").fields().forEachRemaining(e -> {
                var n = e.getValue();
                var resources = new HashSet<String>(); n.get("resources").forEach(r -> resources.add(r.textValue()));
                operations.put(e.getKey(), new Operation(n.get("entry").textValue(), n.get("direction").textValue(),
                        n.get("input").textValue(), n.hasNonNull("output") ? n.get("output").textValue() : null, resources));
            });
            return new TrustedExtensionCatalog(pin, artifact, operations);
        } catch (java.io.IOException e) { throw new IllegalArgumentException("invalid catalog JSON", e); }
        catch (NoSuchAlgorithmException e) { throw new AssertionError(e); }
    }
    private static Set<String> keys(JsonNode node) {
        if (node == null || !node.isObject()) throw bad("expected catalog record");
        var keys = new HashSet<String>();
        node.fieldNames().forEachRemaining(key -> { Codec.encodeProfile(key); keys.add(key); }); return keys;
    }
    private static void fields(JsonNode node, Set<String> required, Set<String> optional) {
        var keys = keys(node); var allowed = new HashSet<>(required); allowed.addAll(optional);
        if (!keys.containsAll(required) || !allowed.containsAll(keys)) throw bad("unknown/missing catalog field");
    }
    private static String text(JsonNode node) {
        if (node == null || !node.isTextual()) throw bad("expected catalog text");
        Codec.encodeProfile(node.textValue()); return node.textValue();
    }
    private static void name(String name, String namespace) {
        Codec.validate("common::capability-name", name);
        if (!name.startsWith(namespace + ".")) throw bad("extension name outside namespace");
    }
    private static Set<String> texts(JsonNode node) {
        if (node == null || !node.isArray()) throw bad("expected catalog array");
        var values = new HashSet<String>();
        for (var value : node) if (!values.add(text(value))) throw bad("duplicate catalog value");
        return values;
    }
    private static void validateDocument(JsonNode a) {
        fields(a, Set.of("version", "namespace", "entries", "schemas", "operations", "rules"), Set.of());
        if (!a.get("version").isIntegralNumber() || !a.get("version").bigIntegerValue().equals(BigInteger.ONE))
            throw bad("unsupported catalog version");
        String ns = text(a.get("namespace"));
        if (!ns.matches("[a-z][a-z0-9_]{0,31}") || Set.of("runtime", "speech", "expression", "llm", "action", "hub",
                "test", "sim", "sensor", "motor", "other").contains(ns)) throw bad("reserved/invalid namespace");
        for (String part : List.of("entries", "schemas", "operations", "rules")) keys(a.get(part));
        if (a.get("operations").isEmpty() || a.get("rules").isEmpty()) throw bad("empty catalog");
        a.get("entries").fields().forEachRemaining(e -> {
            name(e.getKey(), ns); for (String resource : texts(e.getValue())) name(resource, ns);
        });
        a.get("schemas").fields().forEachRemaining(e -> {
            String uri = e.getKey();
            if (!uri.startsWith("https://") || uri.length() > 2048 || uri.contains("coglet.dev/spec/messages/")
                    || uri.codePoints().anyMatch(c -> Character.isWhitespace(c) || Character.isSpaceChar(c) || c == 0x85)
                    || !a.get("rules").has(text(e.getValue())))
                throw bad("unknown/reserved schema URI/rule");
        });
        var heights = new HashMap<String,Integer>();
        for (String rule : keys(a.get("rules"))) ruleHeight(a.get("rules"), rule, new HashSet<>(), heights, 0);
        a.get("operations").fields().forEachRemaining(e -> {
            name(e.getKey(), ns); var op = e.getValue();
            fields(op, Set.of("entry", "direction", "input", "resources", "single_owner"), Set.of("output", "selection"));
            String entry = text(op.get("entry")), direction = text(op.get("direction"));
            if (!a.get("entries").has(entry) || !Set.of("read", "event", "action").contains(direction)
                    || !op.get("single_owner").isBoolean() || !op.get("single_owner").booleanValue()
                    || !a.get("schemas").has(text(op.get("input")))) throw bad("invalid extension binding");
            if (direction.equals("event")) {
                if (op.has("output")) throw bad("event output must be omitted");
            } else if (!a.get("schemas").has(text(op.get("output")))) throw bad("missing output schema");
            if (!texts(a.get("entries").get(entry)).containsAll(texts(op.get("resources"))))
                throw bad("unprovisioned operation resource");
            if (op.has("selection")) {
                var s = op.get("selection");
                fields(s, Set.of("operation_field", "args_field", "value_field", "reads"), Set.of());
                var selectedFields = new HashSet<String>();
                for (String field : List.of("operation_field", "args_field", "value_field")) {
                    String key = text(s.get(field));
                    if (key.isEmpty() || !selectedFields.add(key)) throw bad("invalid selected-read fields");
                }
                Set<String> reads = texts(s.get("reads"));
                if (!direction.equals("event") || reads.isEmpty()) throw bad("selection requires event and nonempty reads");
                for (String read : reads) {
                    var binding = a.get("operations").get(read);
                    if (binding == null || !"read".equals(text(binding.get("direction")))) throw bad("unknown selected read");
                }
            } else if (e.getKey().equals("game.observation")) throw bad("game.observation requires selected-read binding");
        });
    }
    private static BigInteger integer(JsonNode node) {
        if (node == null || !node.isIntegralNumber()) throw bad("expected IR integer");
        BigInteger n = node.bigIntegerValue(); Codec.encodeProfile(n); return n;
    }
    private static int ruleHeight(JsonNode rules, String name, Set<String> stack, Map<String,Integer> heights, int depth) {
        if (depth > 64) throw bad("extension IR nesting");
        if (heights.containsKey(name)) {
            int height = heights.get(name);
            if (depth + height > 64) throw bad("extension IR nesting");
            return height;
        }
        if (!rules.has(name) || !stack.add(name)) throw bad("recursive/unknown extension IR reference");
        int height = closedIr(rules, rules.get(name), stack, heights, depth);
        stack.remove(name); heights.put(name, height); return height;
    }
    private static int closedIr(JsonNode rules, JsonNode node, Set<String> stack, Map<String,Integer> heights, int depth) {
        if (depth > 64) throw bad("extension IR nesting");
        keys(node); String opcode = text(node.get("op")); int height = 0;
        switch (opcode) {
            case "ref" -> {
                fields(node, Set.of("op", "name"), Set.of());
                height = 1 + ruleHeight(rules, text(node.get("name")), stack, heights, depth + 1);
            }
            case "record" -> {
                fields(node, Set.of("op", "fields"), Set.of());
                if (!node.get("fields").isArray()) throw bad("expected IR fields");
                var seen = new HashSet<String>();
                for (var field : node.get("fields")) {
                    fields(field, Set.of("name", "optional", "type"), Set.of());
                    if (!seen.add(text(field.get("name"))) || !field.get("optional").isBoolean()) throw bad("invalid/duplicate IR field");
                    height = Math.max(height, 1 + closedIr(rules, field.get("type"), stack, heights, depth + 1));
                }
            }
            case "union", "intersection" -> {
                fields(node, Set.of("op", "items"), Set.of());
                if (!node.get("items").isArray() || node.get("items").isEmpty()) throw bad("empty IR alternatives");
                for (var item : node.get("items")) height = Math.max(height, 1 + closedIr(rules, item, stack, heights, depth + 1));
            }
            case "array" -> {
                fields(node, Set.of("op", "min", "max", "item"), Set.of());
                BigInteger min = integer(node.get("min")), max = integer(node.get("max"));
                if (min.signum() < 0 || max.compareTo(min) < 0 || max.compareTo(BigInteger.valueOf(262144)) > 0)
                    throw bad("unbounded IR array");
                height = 1 + closedIr(rules, node.get("item"), stack, heights, depth + 1);
            }
            case "primitive" -> {
                fields(node, Set.of("op", "name"), Set.of());
                if (!Set.of("tstr", "bool", "null").contains(text(node.get("name")))) throw bad("unknown IR primitive");
            }
            case "range", "size" -> {
                fields(node, Set.of("op", "min", "max"), Set.of());
                BigInteger min = integer(node.get("min")), max = integer(node.get("max"));
                if (max.compareTo(min) < 0 || (opcode.equals("size") && min.signum() < 0)) throw bad("invalid IR bounds");
            }
            case "literal" -> {
                fields(node, Set.of("op", "value"), Set.of()); var value = node.get("value");
                // Generated literal IR is scalar; the Java codec does not implement
                // container literals. Reject rather than silently misvalidate them.
                if (value.isIntegralNumber()) integer(value);
                else if (value.isTextual()) text(value);
                else if (!value.isBoolean() && !value.isNull()) throw bad("unsupported IR literal");
            }
            case "regexp" -> {
                fields(node, Set.of("op", "pattern"), Set.of());
                portableRegex(text(node.get("pattern")));
            }
            default -> throw bad("unsupported/open extension IR opcode");
        }
        if (depth + height > 64) throw bad("extension IR nesting");
        return height;
    }
    /** Conservative common Java/Rust language: ASCII literal/class atoms, optional
     * edge anchors, finite repetitions, and [\s\S] for all Unicode. No groups,
     * alternatives, dialect escapes, dot or unbounded/backtracking repetition.
     * At most one variable-width atom avoids combinatorial ambiguous partitions.
     */
    private static void portableRegex(String pattern) {
        if (pattern.length() > 4096 || pattern.chars().anyMatch(c -> c < 32 || c > 126)) throw bad("nonportable regexp");
        int end = pattern.length(), i = 0, variable = 0;
        if (end > 0 && pattern.charAt(0) == '^') i++;
        if (end > i && pattern.charAt(end - 1) == '$' && (end < 2 || pattern.charAt(end - 2) != '\\')) end--;
        while (i < end) {
            char c = pattern.charAt(i++);
            if (c == '[') {
                if (pattern.startsWith("[\\s\\S]", i - 1)) i += 5;
                else {
                    if (i < end && pattern.charAt(i) == '^') i++;
                    int atoms = 0;
                    while (i < end && pattern.charAt(i) != ']') {
                        char item = pattern.charAt(i++);
                        if (item == '[' || item == '&') throw bad("nonportable regexp class");
                        if (item == '\\') {
                            if (i >= end || !"\\[]^-".contains(String.valueOf(pattern.charAt(i++)))) throw bad("nonportable regexp escape");
                        }
                        atoms++;
                    }
                    if (atoms == 0 || i >= end) throw bad("invalid regexp class");
                    i++;
                }
            } else if (c == '\\') {
                if (i >= end || !"\\.[]{}()?*+^$|-".contains(String.valueOf(pattern.charAt(i++)))) throw bad("nonportable regexp escape");
            } else if (".()|?*+{}^$]".indexOf(c) >= 0) throw bad("nonportable regexp atom");
            if (i < end && pattern.charAt(i) == '{') {
                int close = pattern.indexOf('}', i);
                if (close < 0 || close >= end) throw bad("invalid regexp repetition");
                String bounds = pattern.substring(i + 1, close);
                if (!bounds.matches("[0-9]{1,6}(,[0-9]{1,6})?")) throw bad("unbounded regexp repetition");
                String[] parts = bounds.split(","); int min = Integer.parseInt(parts[0]);
                int max = parts.length == 1 ? min : Integer.parseInt(parts[1]);
                if (min > max || max > 262144 || (min != max && ++variable > 1)) throw bad("ambiguous/large regexp repetition");
                i = close + 1;
            } else if (i < end && pattern.charAt(i) == '?') {
                if (++variable > 1) throw bad("ambiguous regexp repetition");
                i++;
            }
        }
        java.util.regex.Pattern.compile(pattern);
    }
    public String pin() { return pin; }
    public Operation operation(String name, String direction) {
        var op = operations.get(name);
        if (op == null || !op.direction().equals(direction)) throw bad("unknown operation/direction");
        return op;
    }
    public void validateInput(String name, String direction, Object value) {
        validate(operation(name, direction).input(), value);
        if (direction.equals("event")) requiredOperations(name, value);
    }
    /** Schema-derived conjunction only, NOT a grant or an authorization verdict.
     * Acquire both grants before native materialization, then recheck the same
     * immutable token after encoding and atomically at output admission.
     */
    public Set<String> requiredOperations(String event, Object payload) {
        var op = operation(event, "event"); validate(op.input(), payload);
        var selection = artifact.get("operations").get(event).get("selection");
        if (selection == null) return Set.of(event);
        if (!(payload instanceof Map<?,?> map)) throw bad("expected selected-read wrapper");
        Object name = map.get(selection.get("operation_field").textValue());
        if (!(name instanceof String read) || !texts(selection.get("reads")).contains(read)) throw bad("unsupported selected read");
        validateInput(read, "read", map.get(selection.get("args_field").textValue()));
        validateOutput(read, "read", map.get(selection.get("value_field").textValue()));
        return Set.of(event, read);
    }
    public void validateOutput(String name, String direction, Object value) {
        String uri = operation(name, direction).output();
        if (uri == null) throw bad("event has no output schema");
        validate(uri, value);
    }
    private void validate(String uri, Object value) {
        Codec.validateWith(artifact.get("rules"), artifact.get("schemas").get(uri).textValue(), value);
    }
}
