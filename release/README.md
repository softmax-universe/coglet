# Release tooling

`assemble.py` validates version tags, archives committed source snapshots, stages
a versioned Python build, checks binary architecture and executable permissions,
and seals the exact complete asset inventory with hashes. It never publishes.
`wheel_smoke.py` runs with the separately installed release wheel, not the source
checkout. Run regressions with `python3 -m unittest discover -s release -v`.

The release workflow owns builds and publication. See
[`docs/releasing.md`](../docs/releasing.md) for artifact contracts, immutable-tag
rules, consumer pins and explicit reference-engine/platform qualification limits.

The pipeline requires the real `coglet-hub` binary from the kernel producer;
never supply a stub to satisfy the inventory. Source archives preserve the
complete workspace so Rust path dependencies and harness adapter paths remain
valid. Private Rust crates are not uploaded to crates.io.
