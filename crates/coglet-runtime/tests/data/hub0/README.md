# Frozen serving hub0 transfer artifacts

`fetch.json` is the byte-identical historical Git-transfer corpus from
`ac4e0daf^:spec/fixtures/hub/fetch.json` (Git blob
`90a1beeeaaaa7b41a5d81e683e1143230a6fcaea`). The current manage-protocol spec
removed that file, but the serving runtime receiver still implements its private
frozen hub0 protocol. Keep its qualification artifacts with that receiver.

These are signed Git bundles and archive digests, not ABI filesystem state
hashes. They are intentionally unchanged by ADR-0035; no old trace is rehashed.
The receiver fixture installs the final logs snapshot and the independent Python
oracle checks the exact granted transfer metadata against the same frozen input.
