# iroh conformance driver

This is a byte-transparent test transport, not a router/runtime implementation.
It never parses CBOR, enrollment, grants or fixture results. No new protocol
fixture labels are advertised by adding these controls.

```sh
cargo test --locked --manifest-path conformance/iroh-driver/Cargo.toml
cargo clippy --locked --manifest-path conformance/iroh-driver/Cargo.toml --all-targets -- -D warnings
```

## Connection direction

The default `--mode dial` retains the existing behavior: `--endpoint` pins the
remote public key; optional `--address` is the peer's direct IP:port. Without it,
iroh's N0 discovery is used. Test-local direct connections use Minimal endpoints.

`--mode accept --endpoint <pinned-device-id> --address 127.0.0.1:0 --alpn <alpn>`
binds a loopback listener with that exact ALPN. The first accepted connection
must authenticate as the pinned device. An unexpected identity closes the
connection with code 1 and fails the driver; it is not silently accepted or
retried. This supports a **device-opened** connection whose responder later opens
reverse operation streams. It does not make a production runtime an inbound
server or implicitly negotiate enrollment.

The bound listener is published on stderr as one line:

```
COGLET_IROH_LISTENING <local-public-key> <bound-loopback-IP:port>
```

A test launcher can read this before dialing. The Python registry supports
`iroh_mode = "accept"` with a fixed local `address`; its normal stderr collector
does not dynamically rewrite registry addresses from the discovery line.
`--key-file` or `COGLET_FIXTURE_IROH_KEY_FILE` pins the local fixture identity.
Never put production secrets in a case or registry.

## Operation direction and routing

stdin is a sequence of u32be-framed controls. Each body contains an 8-byte
big-endian tooling stream ID, a one-byte `finish` flag (0 or 1), then the exact
wire frame body. The tooling prefix is **not** sent over QUIC. The first send for
an unused ID opens a bidi stream; later sends use the same stream. A finished ID
cannot be reused. stdout retains the existing u32be-framed wire bodies, without
a tooling prefix. uint64 values and arbitrary payload bytes are not interpreted.

`--incoming-stream 100` (registry `incoming_stream = 100`) reserves that tooling
ID for the first peer-initiated bidi stream on this connection. The send half is
registered before its request is forwarded to stdout. Replies/chunks with ID 100
therefore return on that exact stream, interleaved with independently opened
outbound operations. Sending on the reserved ID before the request arrives,
reusing it after FIN, or ending stdin before it arrives is an error.

This option maps **one** incoming operation, not arbitrary concurrent inbound
RPCs. It cannot identify the operation semantically: the fixture must arrange
which stream arrives first. Additional inbound streams are not serviced. A
fixture launcher owns enrollment and peer-specific setup; the driver does not
invent an enrollment reply or translate request tokens.

The driver keeps partial stdin reads alive while accepting the incoming stream
and reaping operations. Bounds are 1 MiB wire frames, 64 concurrent receive tasks,
64 outstanding FIN acknowledgements, 10,000 distinct tooling IDs and a 300-second
transport watchdog. EOF with any unfinished send is an error. On clean EOF it
waits for remote read FINs and acknowledgement of all locally finished sends
before closing the connection. QUIC acknowledgement proves byte delivery, **not**
application processing, durable import or a passing fixture oracle. Resets and
transport failures fail by default. An isolated negative fixture may opt in to
`--expect-reset-stream 100` (or `COGLET_FIXTURE_EXPECT_RESET_STREAM=100`). Only an
actual code-1 reset at a frame boundary on that exact tooling stream qualifies;
clean FIN, other codes, connection loss and partial framing still fail. The
driver retires that operation's unfinished send with a code-1 reset. It forwards
all preceding frames, so the harness terminal drain rejects any unexpected
credit/disclosure before the reset. This is transport observation, not the local
policy-conflict or durable-state oracle; both remain fixture responsibilities.

## Post-wire verification

The Python harness closes iroh stdin at timeline end and requires clean stdout
EOF plus driver exit 0 under the whole-case watchdog. This prevents a final
send-only step from passing merely because bytes reached a pipe. Trailing
unconsumed output is a failure. Fixtures may additionally register `verify` argv
hooks, executed after wire completion and before teardown. A nonzero verifier
fails the case even when every wire match passed. Such hooks must inspect actual
terminal/durable state; a driver component test is not a publication, inbox,
audit, rollback or full router-conformance proof.
