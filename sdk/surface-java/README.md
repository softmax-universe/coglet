# Java surface message SDK

Java 21 generated records and sealed interfaces for direct record unions, with
native strict deterministic CBOR and runtime CDDL validation. Jackson is used only
for the generated schema resource and JSON fixtures, never for CBOR.

```java
import org.coglet.surface.Generated;
import org.coglet.surface.Codec;

var offer = new Generated.VersionOffer(java.util.List.of("0.1"));
byte[] bytes = offer.toCbor();
var decoded = Generated.VersionOffer.fromCbor(bytes);
Object generic = Codec.decode("version-offer", bytes);
```

All integer fields use `BigInteger`; native wire integers cover `-2^63` through
`2^64-1`. Individual schema rules can impose narrower ranges. Generic values are
`null`, `Boolean`, `String`, `BigInteger` (integral Java primitives are accepted
when encoding), `List<?>`, and text-keyed `Map<?,?>`. No bytes, floats, tags,
indefinite forms, non-shortest lengths, trailing bytes, duplicate/unsorted or
nontext keys, or malformed Unicode are accepted. Nesting is bounded to 64 and
schema traversal to 128.

Optional generated components use `Generated.Presence<T>`: a null component means
absent, and `new Generated.Presence<>(null)` means explicitly present null (only
accepted if its CDDL type permits null). Required null fields remain present.

`Generated.Message.toCbor()` validates both the rule and concrete record branch;
`fromCbor()` validates before constructing nested records. Generic
`Codec.encode(rule, value)` / `decode(rule, bytes)` cover every rule, including
recursive profile values. `encodeProfile` / `decodeProfile` expose only the wire
profile without schema constraints. These are message codecs, not a transport
client or a claim of full protocol conformance.

Run `mvn test` with JDK 21. Tests recursively walk `../../spec/fixtures`, assert
minimum corpus counts (33 golden, 43 semantic-invalid, 17 wire-invalid), and check
byte-identical roundtrips, typed records, wrong variants, integer bounds,
Unicode, container lengths, wire depth, and schema recursion. Generation is
integrated through `codegen.emit_java.generate(rules)`; generated Java and schema
resources must not be edited by hand.
