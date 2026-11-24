package org.coglet.surface;

import static org.junit.jupiter.api.Assertions.*;
import java.util.*;
import org.junit.jupiter.api.*;

class TrustedExtensionCatalogTest {
    static final String PIN = "2f3ab80d5b09134e35a2e63e5a3c14e314e61ac12135fa0a1c723aaf02cf79dd";
    static byte[] fixture() throws Exception {
        try (var in = TrustedExtensionCatalogTest.class.getResourceAsStream("/trusted-extension-catalog.json")) {
            return in.readAllBytes();
        }
    }
    static com.fasterxml.jackson.databind.node.ObjectNode document() throws Exception {
        return (com.fasterxml.jackson.databind.node.ObjectNode) new com.fasterxml.jackson.databind.ObjectMapper().readTree(fixture());
    }
    static TrustedExtensionCatalog load(com.fasterxml.jackson.databind.JsonNode document) throws Exception {
        return loadRaw(new com.fasterxml.jackson.databind.ObjectMapper().writeValueAsBytes(document));
    }
    static TrustedExtensionCatalog loadRaw(byte[] raw) throws Exception {
        String pin = java.util.HexFormat.of().formatHex(java.security.MessageDigest.getInstance("SHA-256").digest(raw));
        return TrustedExtensionCatalog.fromPinnedBytes(raw, pin);
    }
    @Test void utf8OnlyClosedJsonRejectsAlternateEncodingDuplicateAndTrailingBytes() throws Exception {
        String json = new String(fixture(), java.nio.charset.StandardCharsets.UTF_8);
        for (byte[] invalid : List.of(
                json.getBytes(java.nio.charset.StandardCharsets.UTF_16),
                ("\ufeff" + json).getBytes(java.nio.charset.StandardCharsets.UTF_8),
                json.replace("\"version\": 1", "\"version\": 1, \"version\": 1").getBytes(java.nio.charset.StandardCharsets.UTF_8),
                (json + " {}").getBytes(java.nio.charset.StandardCharsets.UTF_8),
                new byte[1_048_577])) {
            assertThrows(IllegalArgumentException.class, () -> loadRaw(invalid));
        }
    }
    @Test void closedProvisioningBindingsRefuseMisconfiguredAuthority() throws Exception {
        for (String field : List.of("version", "namespace", "entries", "schemas", "rules", "operations")) {
            var doc = document(); doc.remove(field);
            assertThrows(IllegalArgumentException.class, () -> load(doc), "missing " + field);
        }
        var unknown = document(); unknown.put("unknown", true);
        assertThrows(IllegalArgumentException.class, () -> load(unknown), "unknown top-level field");
        for (String ns : List.of("speech", "expression", "runtime", "test", "sim", "sensor", "motor", "other", "bad.ns", "")) {
            var doc = document(); doc.put("namespace", ns);
            assertThrows(IllegalArgumentException.class, () -> load(doc), "namespace " + ns);
        }
        for (String field : List.of("entry", "direction", "input", "output", "resources", "single_owner", "unknown")) {
            var doc = document();
            var op = (com.fasterxml.jackson.databind.node.ObjectNode) doc.path("operations").path("game.move");
            switch (field) {
                case "entry" -> op.put(field, "game.missing");
                case "direction" -> op.put(field, "reverse");
                case "input", "output" -> op.put(field, "https://example.invalid/missing");
                case "resources" -> op.putArray(field).add("game.unprovisioned");
                case "single_owner" -> op.put(field, false);
                default -> op.put(field, true);
            }
            assertThrows(IllegalArgumentException.class, () -> load(doc), "binding " + field);
        }
    }
    @Test void closedFiniteIrRejectsOpenRecursiveAndUnknownShapesAtLoad() throws Exception {
        var json = new com.fasterxml.jackson.databind.ObjectMapper();
        for (String invalid : List.of(
                "{\"op\":\"record\",\"fields\":[],\"rest\":{\"op\":\"primitive\",\"name\":\"tstr\"}}",
                "{\"op\":\"map\"}", "{\"op\":\"primitive\",\"name\":\"float\"}",
                "{\"op\":\"ref\",\"name\":\"missing\"}", "{\"op\":\"ref\",\"name\":\"empty\"}",
                "{\"op\":\"union\",\"items\":[]}", "{\"op\":\"intersection\",\"items\":[]}",
                "{\"op\":\"array\",\"min\":0,\"max\":262145,\"item\":{\"op\":\"literal\",\"value\":true}}",
                "{\"op\":\"range\",\"min\":9,\"max\":1}", "{\"op\":\"size\",\"min\":-1,\"max\":5}",
                "{\"op\":\"regexp\",\"pattern\":\"[\"}",
                "{\"op\":\"literal\",\"value\":1.5}", "{\"op\":\"literal\",\"value\":{}}",
                "{\"op\":\"literal\",\"value\":9223372036854775808,\"ignored\":true}")) {
            var doc = document(); ((com.fasterxml.jackson.databind.node.ObjectNode) doc.path("rules")).set("empty", json.readTree(invalid));
            assertThrows(IllegalArgumentException.class, () -> load(doc), invalid);
        }
        var doc = document();
        var rules = (com.fasterxml.jackson.databind.node.ObjectNode) doc.path("rules");
        rules.set("empty", json.readTree("{\"op\":\"ref\",\"name\":\"cycle\"}"));
        rules.set("cycle", json.readTree("{\"op\":\"ref\",\"name\":\"empty\"}"));
        assertThrows(IllegalArgumentException.class, () -> load(doc));
    }
    static Map<String,Object> observation(String operation, Map<String,Object> args, Map<String,Object> value) {
        return Map.of("query_id", "1".repeat(32), "operation", operation, "args", args, "value", value,
                "revision", 1, "world_generation", 1, "grant_epoch", 1);
    }
    @Test void selectedReadValidatesBothSiblingsAndReturnsConjoinedAuthority() throws Exception {
        var catalog = TrustedExtensionCatalog.fromPinnedBytes(fixture(), PIN);
        var valid = observation("game.state", Map.of(), Map.of("paused", true));
        catalog.validateInput("game.observation", "event", valid);
        assertEquals(Set.of("game.observation", "game.state"), catalog.requiredOperations("game.observation", valid));
        // The broad wrapper permits both sibling types; selection must not.
        assertThrows(IllegalArgumentException.class, () -> catalog.validateInput("game.observation", "event",
                observation("game.state", Map.of("tile", 7), Map.of("paused", true))));
        assertThrows(IllegalArgumentException.class, () -> catalog.validateInput("game.observation", "event",
                observation("game.state", Map.of(), Map.of("count", 3))));
        assertThrows(IllegalArgumentException.class, () -> catalog.validateInput("game.observation", "event",
                observation("game.move", Map.of(), Map.of("paused", true))));
        final var missingSelection = document(); ((com.fasterxml.jackson.databind.node.ObjectNode) missingSelection.path("operations").path("game.observation")).remove("selection");
        assertThrows(IllegalArgumentException.class, () -> load(missingSelection));
        for (String field : List.of("operation_field", "args_field", "value_field", "reads", "unknown")) {
            var doc = document();
            var selection = (com.fasterxml.jackson.databind.node.ObjectNode) doc.path("operations").path("game.observation").path("selection");
            if (field.equals("reads")) selection.putArray(field).add("game.move");
            else selection.put(field, "");
            final var changed = doc;
            assertThrows(IllegalArgumentException.class, () -> load(changed), field);
        }
    }
    @Test void nonportableRegexAndInvalidUnicodeConfigurationFailClosed() throws Exception {
        for (String pattern : List.of(".*", "\\d", "\\w", "(?=a)a", "(a)\\1", "a*+", "(.*a){12}", "é", "[a-z&&[^b]]")) {
            var doc = document(); ((com.fasterxml.jackson.databind.node.ObjectNode) doc.path("rules").path("id")).put("pattern", pattern);
            assertThrows(IllegalArgumentException.class, () -> load(doc), pattern);
        }
        for (String suffix : List.of("\u00a0", "\u0085", "\u2007", "\u202f")) {
            var doc = document(); ((com.fasterxml.jackson.databind.node.ObjectNode) doc.path("schemas"))
                    .put("https://example.invalid/extra" + suffix, "empty");
            assertThrows(IllegalArgumentException.class, () -> load(doc), "whitespace URI");
        }
        var doc = document(); ((com.fasterxml.jackson.databind.node.ObjectNode) doc.path("rules"))
                .set("\ud800", doc.path("rules").path("empty").deepCopy());
        assertThrows(IllegalArgumentException.class, () -> load(doc), "malformed Unicode key");
    }
    @Test void portableRegexpVectorsExerciseRealSelectedSchemaValidation() throws Exception {
        var json = new com.fasterxml.jackson.databind.ObjectMapper();
        var vectors = json.readTree(TrustedExtensionCatalogTest.class.getResourceAsStream("/portable-regexp-vectors.json"));
        for (var vector : vectors) {
            var doc = document();
            var query = (com.fasterxml.jackson.databind.node.ObjectNode) doc.path("rules").path("tile-query");
            var type = (com.fasterxml.jackson.databind.node.ObjectNode) query.path("fields").get(0).path("type");
            type.removeAll(); type.put("op", "regexp"); type.put("pattern", vector.path("pattern").textValue());
            var catalog = load(doc);
            for (var value : vector.path("values")) {
                var args = Map.<String,Object>of("tile", value.path("text").textValue());
                if (value.path("valid").booleanValue()) catalog.validateInput("game.move", "action", args);
                else assertThrows(IllegalArgumentException.class, () -> catalog.validateInput("game.move", "action", args));
            }
        }
    }
    @Test void exactRuntimeArtifactPinSelectsLocalSiblingSchemas() throws Exception {
        var catalog = TrustedExtensionCatalog.fromPinnedBytes(fixture(), PIN);
        assertEquals(PIN, catalog.pin());
        var op = catalog.operation("game.state", "read");
        assertEquals("game.view", op.entry());
        assertEquals(Set.of(), op.resources());
        catalog.validateInput("game.state", "read", Map.of());
        catalog.validateOutput("game.state", "read", Map.of("paused", true));
        assertThrows(IllegalArgumentException.class,
                () -> catalog.validateOutput("game.state", "read", Map.of("count", 1)));
        assertThrows(IllegalArgumentException.class, () -> catalog.operation("game.state", "action"));
        assertThrows(IllegalArgumentException.class, () -> catalog.operation("speech.stop", "action"));
        assertThrows(IllegalArgumentException.class,
                () -> TrustedExtensionCatalog.fromPinnedBytes(fixture(), "0".repeat(64)));
        assertThrows(IllegalArgumentException.class, () -> TrustedExtensionCatalog.fromPinnedBytes(fixture(), PIN.toUpperCase()));
    }
}
