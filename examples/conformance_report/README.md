# Offline typing-conformance report

This is a typed port of Ruff's complete conformance comparison and reporting
tool. It scans the Python typing conformance suite's raw annotations, decodes
two ty GitLab JSON outputs, groups required/optional/tagged/multi annotations,
classifies diagnostics, computes all per-case and per-file metrics, and renders
the same Markdown or GitHub HTML report as the pinned upstream implementation.

The unchanged [upstream source](upstream/conformance.py), its MIT license and
[provenance](upstream/UPSTREAM.json) are retained. The report algorithm is from
Ruff commit `de9071547cfec12f689142a7b8ce98edc7aef909`.

## Run a real comparison

Prepare a checkout of [python/typing](https://github.com/python/typing) at
`835b70f3f795f8e061b1f7621178515b0a77a0ad` and two ty executables. The preparation
step runs ty with the upstream flags and records the source/binary hashes,
commands, explicit environment, and raw stdout/stderr. Use a new output directory.

```sh
python3 examples/conformance_report/prepare.py \
  --suite /absolute/path/to/typing/conformance \
  --old-ty /absolute/path/to/old/ty --new-ty /absolute/path/to/new/ty \
  --output-dir /tmp/conformance-input

relar build examples/conformance_report/__main__.py --module-root examples \
  -o /tmp/conformance-report

/tmp/conformance-report --manifest /tmp/conformance-input/manifest.json \
  --old /tmp/conformance-input/old.json --new /tmp/conformance-input/new.json \
  --format github --output /tmp/conformance.md
```

`--format diff` renders local-path diagnostic diffs. `--force-summary-table`
includes the full metrics table even when diagnostics are unchanged. Without
`--output`, the report is printed to stdout with a final newline. With an output
file, all parsing and rendering finish before the file is opened; successful
file output is quiet. The file's bytes match the upstream report exactly.

The port can also run on CPython:

```sh
PYTHONPATH=examples:stdlib python3 -m conformance_report \
  --manifest /tmp/conformance-input/manifest.json \
  --old /tmp/conformance-input/old.json --new /tmp/conformance-input/new.json
```

## Adaptations and scope

The CLI consumes a manifest and captured JSON rather than launching ty. The
manifest contains `suite_revision`, the selected `test_files`, and
`diagnostic_paths` entries with `raw` and canonical `path` strings. Preparation
does test-group discovery and path resolution; it does not parse annotations,
decode diagnostics into report records, classify cases, calculate metrics, or
render reports for the measured program. Test paths are the already resolved
absolute identities produced by preparation. Inputs must use UTF-8 and complete
GitLab diagnostic records with integer positions; unknown JSON fields are ignored.

String tags replace enums, ordinary display helpers replace `__str__`, and
concrete records replace dynamic JSON dictionaries. A linear scanner implements
the upstream regex, including Unicode whitespace, scalar column offsets, first
match behavior, and malformed-tag fallback. Assertions for no annotations and
no new true positives remain errors; the latter omits the upstream diagnostic
record debug dump. The adapter suppresses the upstream output-file confirmation
and duplicated report on stderr.

`original.py` runs the unchanged upstream regex, models and renderers with the
same offline inputs and quiet file-output adapter. It is explicitly an upstream
**adapter**, not the unchanged upstream CLI. Its preloaded source wrapper and
canonical paths remove repeated discovery and path resolution for all engines.

`compute.py` repeats the complete annotation scan, JSON decode, grouping,
classification, sorting and rendering over preloaded raw text, then prints an
output-length checksum. With `--output`, it also writes the complete final
report once after the loop, so benchmark gates can check every byte. Zero rounds
write an empty report and print `0`; `--rounds 0` is the matching preload/startup
control. The upstream adapter exposes this as `--compute --rounds N`.
Preparation and input acquisition are outside both compute and report timings;
ordinary report timings still include all filesystem reads and output writes.

```sh
relar build examples/conformance_report/compute.py --module-root examples \
  -o /tmp/conformance-compute
/tmp/conformance-compute --manifest /tmp/conformance-input/manifest.json \
  --old /tmp/conformance-input/old.json --new /tmp/conformance-input/new.json \
  --format diff --force-summary-table --rounds 8 --output /tmp/conformance.md
```

The real comparison inputs select 146 tests from the pinned typing suite. The
ty 0.0.89 → 0.0.90 captures are identical (1,201 diagnostics each); ty 0.0.60 →
0.0.90 exercises changed reports (1,169 → 1,201 diagnostics) under the exact same
upstream command and lint policy. Both raw captures, including their original
diagnostic paths, are retained unchanged. The validator's `materialize_real`
helper copies the selected source bytes and rebases the manifest's canonical
identities for offline replay; preparation records bind the original source,
tool and stream hashes. Retain the typing checkout's root `LICENSE` alongside
the captured inputs when sharing them.

`benchmarks/conformance_validate.py` compares full reports and filesystem effects
against the unchanged upstream implementation. Its seeded checks exercise the
annotation scanner independently and cover every diagnostic classification,
tag grouping, optional/multi annotations, Unicode, line endings, integer widths,
both formats, summary-table modes, and failing inputs. The independent Rust
reference lives in `benchmarks/reference/conformance_report`; native Codon
adapters and their explicitly bounded input domain live in
`benchmarks/codon_native`. Parenthesized sequence patterns in the shared port
avoid a Codon distinction between tuple and bracketed patterns; their Python
AST is identical.
