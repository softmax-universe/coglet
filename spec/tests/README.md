# Surface/speech fixture validation

Install the existing Python requirements using Python 3.12 (pycddl 0.6.4 does
not build on Python 3.14). Also install Ruby 3.2 and Bundler 2.5.23, then:

```sh
bundle install --gemfile spec/tests/Gemfile
python3 -m unittest discover -s spec/tests -v
```

CI installs Ruby dependencies with ruby/setup-ruby and the committed Gemfile.lock.
The new fixture test launches the reference Ruby CDDL validator against actual
imported CDDL, and jsonschema against actual imported JSON Schemas. The common
suite keeps its existing pycddl checks. Do not rewrite/relax common definitions
to make a validator pass: pycddl 0.6.4 and Rust cddl 0.10.7 reject recursive
profile-map values; cddl-cat 0.7.1 cannot parse the int64 minimum bound.
Reference CDDL accepts both. The wrapper distinguishes mismatches (exit 1) from
parser/tool failures (exit 2); missing dependencies fail, never skip.

CDDL validation consumes the lossless JSON fixture view (Ruby preserves uint64)
after Python decodes/checks golden CBOR. It is actual CDDL evaluation, not schema
translation to JSON Schema. Wire/profile/deterministic-order checks remain separate.
Test schemas reject unknown fields and enforce conditional accept/reply shapes;
contextual speech args/payloads are validated in addition to the common envelopes.
UTF-8 byte budgets and temporal authority are additional normative checks, not
claims made by structural schemas alone.

Regenerate after changing the typed profile tables or case definitions:

```sh
python3 spec/tests/build_surface_fixtures.py
python3 spec/tests/build_surface_cases.py
```

Both accept `--check`; the unittest suite checks generated files are current.
`fixtures/speech/timelines.json` is hand-authored native callback data and tested
separately. Real and mock implementation conformance is left to the wire harness
with explicit fixture labels; empty registries and SKIPs are not protocol proof.

The adapter-config, framing and timeline JSON files have adjacent `.json.format`
files containing `semantic-vectors-v1`. This explicitly distinguishes them from
codec case arrays. Their contents remain checked here (including adapter config
against its actual component schemas); a codec-discovery skip is not validation.
