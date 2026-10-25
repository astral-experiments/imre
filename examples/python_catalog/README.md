# Generate the CPython download catalog

This is an offline typed port of the CPython mode in uv's
[`fetch-download-metadata.py`](https://github.com/astral-sh/uv/blob/83d556d712c7e316932b239c49e86a5de95d124a/crates/uv-python/fetch-download-metadata.py).
It turns the raw python-build-standalone release index into the complete JSON
catalog used to select Python downloads. PyPy, GraalPy, and Pyodide's separate
network finders are outside this example.

From the repository root, after building Imre:

```sh
target/release/imre build examples/python_catalog/__main__.py \
  --module-root examples --clang clang-18 -o /tmp/python-catalog
/tmp/python-catalog examples/python_catalog/sample/python-build-standalone.ndjson.gz \
  /tmp/download-metadata.json
```

The retained real index contains 567 release rows and 58,721 artifacts. The
program produces 6,098 catalog entries. A `.gz` input is decompressed inside
the executable. Plain NDJSON uses the same buffered line loop. The program
parses every raw release and artifact, sorts artifact URLs, filters unsupported
builds, selects preferred builds, sorts the final catalog, and renders every
JSON field. The loop retains the current release, selected builds, and up to
512 successfully normalized platform/variant pairs. Uncached pairs follow the
same normalization path after the cache fills.
Published source/input
revisions and hashes are in [upstream/UPSTREAM.json](upstream/UPSTREAM.json) and
[sample/provenance.json](sample/provenance.json).

Run the same typed program on CPython:

```sh
PYTHONPATH=stdlib:examples python3.14 -m python_catalog \
  examples/python_catalog/sample/python-build-standalone.ndjson.gz /tmp/download-metadata.json
```

The port uses fixed dataclasses, typed JSON decoding and encoding, tuple
dictionary keys, and stable tuple-key sorting. It replaces the network request
with a file argument and selects winners while parsing, preserving the upstream
first-insertion order and first-on-equal-priority behavior. Release dates do not
break priority ties. Each release looks up its version group once, creating a
new group when its first artifact passes filtering. Artifacts reuse their
normalized target labels across releases, while the historical musl eligibility
check still uses the date of each release. A winning artifact supplies its own
URL and checksum. Historical static builds,
the musl cutoff, architecture aliases, platform suffix order, prerelease ranking,
and the Python 3.8 floor
follow the retained upstream code. Successful output is quiet and byte-identical
to its `CPythonFinder` plus `render` pipeline on the accepted input domain.
The three version components are checked before unpacking, preserving CPython's
exact `ValueError` messages for too few or too many components. Codon's native
list unpacking otherwise reports a different error or ignores extra components;
all engines execute the same explicit check in this shared program.

The input is UTF-8 NDJSON with a string `version` and an `artifacts` list. Each
artifact has string `url`, `platform`, and `variant` fields; `sha256` is an
optional string or null. Unknown fields are syntax-checked and ignored.
Known-field duplicates, missing required fields, wrong field types, and
unpaired surrogates in known strings are rejected by the typed decoder. JSON
nesting is limited to 128 object/array levels. These checks are stricter than
upstream's dynamic `json.loads` boundary.

An architecture with an empty third underscore component, such as `x86_64_`,
also raises `ValueError`. Upstream's custom `Arch` equality distinguishes an
empty variant from `None`, while its ordering treats them alike; this port
rejects that ambiguous spelling so ordinary tuple ordering stays faithful.
The retained real index contains no such architecture. Other normalization
rules, including ignored extra triple/architecture components, are preserved.

The complete catalog is rendered before opening the output. Decompression,
decoding, and selection failures preserve an existing output. On malformed
records, the reader finishes checking the input so a later I/O or UTF-8 error
retains the upstream bulk reader's precedence. If text decoding fails in a
regular gzip file, a bounded binary pass checks the rest of the framing.
Missing output parents are created, then
the destination is written normally; write failures can leave a partial file.
This matches the upstream renderer's publication behavior.

The [unchanged upstream snapshot](upstream/fetch-download-metadata.py) retains
its Rye MIT attribution and the uv licenses. `upstream_adapter.py` supplies its
one HTTP response by reading the entire plain or compressed input file and calls
the unchanged finder and renderer. The typed port also runs on CPython, giving
a separate measurement of the same streaming algorithm.

The [Variant sort check](../../benchmarks/uv_variant_validate.py) compiles uv’s
original `Variant` and `variant_sort_key`. It sorts the Imre list in place
and reads the result through an alias. CPython supplies the expected order for
all 6,110 downloads passed to the renderer, including ties within each rank. The renderer then filters 12 downloads for Python
versions older than 3.8.

Validate the full catalog, focused edge cases, errors, and complete filesystem
effects against that upstream oracle:

```sh
python3.14 benchmarks/reference/python_catalog/build.py \
  --artifact-dir /tmp/catalog-rust --target-dir /tmp/catalog-rust-target \
  --build-cache /tmp/imre-shared-build
python3.14 benchmarks/catalog_validate.py --binary /tmp/python-catalog \
  --rust /tmp/catalog-rust/python-catalog-reference
```

The builder uses the repository's pinned Rust toolchain by default. On a devbox
with Ohm installed, add `--toolchain ohm` to select it explicitly.

The Rust reference uses typed serde records, stable sorting, insertion-ordered
winner vectors with hash lookup, and native integers with arbitrary-precision
fallback. Its JSON formatter preserves Python's field order, ASCII escaping,
two-space indentation, and trailing newline. The validator compares complete
output bytes rather than a count or checksum alone, and records command and
file identities for later auditing.

The benchmark-only `python_catalog.compute` entrypoint takes `ROUNDS INPUT
OUTPUT`. It preloads raw text once, repeats the entire parse/select/sort/render
pipeline, writes the complete last catalog, and prints the sum of rendered
lengths. Zero rounds writes an empty output and prints zero, providing a preload
control. The unchanged upstream adapter and Rust reference expose the same
arguments after `--compute`; Rust prevents loop hoisting with `black_box`.
Repeated timings include startup, preload, and one final publication; dividing
by rounds gives an amortized estimate. Controls are reported without subtraction.

Pass `--compute` to the validator and supply the corresponding Imre/Codon
compute binaries. It verifies both the complete last output and length sum,
including zero-round behavior, failures, and output aliasing. Codon's native JSON
parser lacks duplicate-key hooks and its native integers are signed 64-bit, so
the validator explicitly records those two adversarial-case exclusions for
Codon. They remain checked for Imre, CPython, and Rust. The complete real catalog
is required to match for every engine.
