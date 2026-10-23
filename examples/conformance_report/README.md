# Typing-conformance report

This is a typed port of Ruff's complete conformance comparison and reporting
tool. It scans the Python typing conformance suite's raw annotations, decodes
two ty GitLab JSON outputs, groups required/optional/tagged/multi annotations,
classifies diagnostics, computes all per-case and per-file metrics, and renders
the same Markdown or GitHub HTML report as the pinned upstream implementation.

The unchanged [upstream source](upstream/conformance.py), its MIT license and
[provenance](upstream/UPSTREAM.json) are retained. The report algorithm is from
Ruff commit `de9071547cfec12f689142a7b8ce98edc7aef909`.

## Run directly from a checkout

Build the reporter, then provide a [python/typing](https://github.com/python/typing)
checkout and two ty executables:

```sh
relar build examples/conformance_report/__main__.py --module-root examples \
  -o /tmp/conformance-report

/tmp/conformance-report --suite /absolute/path/to/typing/conformance \
  --old-ty /absolute/path/to/old/ty --new-ty /absolute/path/to/new/ty \
  --timeout 15.0 --format github --output /tmp/conformance.md
```

The program decodes `src/test_groups.toml` into typed records, discovers and
resolves test sources, invokes both analyzers with the upstream flags, resolves
their diagnostic paths, and renders the report. Each analyzer has its own timeout
in seconds. A sibling temporary directory holds the complete output until an
atomic replacement publishes it. Failures leave an existing report intact and
remove the temporary workspace. The output parent must already exist; successful
file output is quiet. Live mode requires `--output`.

The default report-link revision is the validated typing commit
`835b70f3f795f8e061b1f7621178515b0a77a0ad`. Use `--suite-revision REVISION` when
running another checkout. `--format diff` selects local-path diagnostic diffs;
`--force-summary-table` includes the full metrics table for unchanged results.

To run the typed port on CPython 3.14, install the TOML 1.1 compatibility parser:

```sh
python3.14 -m venv /tmp/conformance-python
/tmp/conformance-python/bin/python -m pip install 'tomli==2.4.0'
PYTHONPATH=examples:stdlib /tmp/conformance-python/bin/python -m conformance_report \
  --suite /absolute/path/to/typing/conformance \
  --old-ty /absolute/path/to/old/ty --new-ty /absolute/path/to/new/ty \
  --timeout 15.0 --output /tmp/conformance.md
```

Native executables use the Rust TOML parser and require no Python installation.
Python 3.15 and later use their standard-library TOML 1.1 parser.

## Replay a captured comparison

From the Relar repository root, replay the included 146-file capture with one
native build and run. The resulting Markdown report compares ty 0.0.60 and 0.0.90.

```sh
workdir=$(mktemp -d)
python3 benchmarks/conformance_grouping_inputs.py --scales 1 \
  --archive benchmarks/results/2026-10-22-collector-ownership-study.tar.xz \
  --output "$workdir/inputs"
panel="$workdir/inputs/1x/changed"
relar run examples/conformance_report/__main__.py --module-root examples -- \
  --manifest "$panel/manifest.json" --old "$panel/old.json" --new "$panel/new.json" \
  --format github --output "$workdir/report.md"
```

On Linux, add `--allocator jemalloc` before the `--` in the `relar run` command
to use the allocator measured in the grouping benchmarks.

To capture a new comparison, prepare a checkout of [python/typing](https://github.com/python/typing) at
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

The offline CLI accepts a manifest and captured JSON. The manifest contains
`suite_revision`, the selected `test_files`, and
`diagnostic_paths` entries with `raw` and canonical `path` strings. Preparation
does test-group discovery and path resolution; it does not parse annotations,
decode diagnostics into report records, classify cases, calculate metrics, or
render reports for the measured program. Test paths are the already resolved
absolute identities produced by preparation. Inputs must use UTF-8 and complete
GitLab diagnostic records with integer positions; unknown JSON fields are ignored.

Relar compiles Ruff’s `group_diagnostics_by_key`, `diagnostics_are_equivalent`,
`collect_file_stats`, `compute_stats`, and `collect_diagnostic_entries` directly.
These functions group diagnostics by path and tag, compare fingerprints, include
files with no cases in the statistics, and sort the final entries. Their source
is checked against the pinned script, along with `ExpectedError.key`,
`TestCase.classify`, `TestCase.change`, `TestCase.diagnostics_by_source`, and
the complete `Source`, `Classification`, and `Change` enums. JSON decoding
uses a separate wire record for the original path string. Each diagnostic and
expected error carries a native `Path` into the analysis.
Ruff’s original verbose regex runs with Rust’s regex engine and exposes the
same named captures and character offsets as CPython. Assertions for no
annotations and no new true positives remain errors. The latter omits the
upstream diagnostic record debug dump. The adapter suppresses the upstream output-file confirmation
and duplicated report on stderr.

`original.py` runs the upstream regex, models, analysis and renderers with
the same offline inputs. The adapter preloads the source text and raw JSON.
Ruff’s file-statistics function resolves every test path on every report pass,
including files with no diagnostics.

`compute.py` repeats the complete annotation scan, JSON decode, grouping,
classification, sorting and rendering over preloaded raw text, then prints an
output-length checksum. With `--output`, it also writes the complete final
report once after the loop, so benchmark gates can check every byte. Zero rounds
write an empty report and print `0`; `--rounds 0` is the matching preload/startup
control. The upstream adapter exposes this as `--compute --rounds N`.
Preparation and input acquisition are outside offline compute and report timings;
offline report timings include all filesystem reads and output writes. Live
timings include TOML, discovery, both actual analyzer processes, path resolution,
report generation, atomic publication, and cleanup. Separate frozen-input
compute timings expose report performance independently of analyzer latency.

```sh
relar build examples/conformance_report/compute.py --module-root examples \
  -o /tmp/conformance-compute
/tmp/conformance-compute --manifest /tmp/conformance-input/manifest.json \
  --old /tmp/conformance-input/old.json --new /tmp/conformance-input/new.json \
  --format diff --force-summary-table --rounds 8 --output /tmp/conformance.md
```

The captured-input comparisons select 146 tests from the pinned typing suite. The
ty 0.0.89 → 0.0.90 captures are identical (1,201 diagnostics each); ty 0.0.60 →
0.0.90 exercises changed reports (1,169 → 1,201 diagnostics) under the exact same
upstream command and lint policy. Both raw captures, including their original
diagnostic paths, are retained unchanged. The validator's `materialize_real`
helper copies the selected source bytes and rebases the manifest's canonical
identities for offline replay; preparation records bind the original source,
tool and stream hashes. Retain the typing checkout's root `LICENSE` alongside
the captured inputs when sharing them.

The live benchmark runs ty 0.0.90 on both sides for its unchanged case and
0.0.60 → 0.0.90 for changed reports. The [checkout comparison](../../benchmarks/results/2026-10-21-live-conformance.md)
includes complete workflow timings, separate report-computation timings,
and reproducible evidence for CPython, Relar, Codon, and Rust.
The [fixed-regex comparison](../../benchmarks/results/2026-10-22-fixed-regex.md)
times Ruff’s original pattern against the previous scanner on the same checkout
and verifies the text and offsets of each captured annotation.
The [string-enum comparison](../../benchmarks/results/2026-10-22-string-enums.md)
measures Ruff’s original `compute_stats` over 1,269 preloaded test groups. It
also times complete and repeated reports and includes Rust controls that
specialize the common one- and two-diagnostic cases.
The [in-place sort comparison](../../benchmarks/results/2026-10-22-entry-sort.md)
measures Ruff’s unchanged diagnostic entry collector and full reports against
the previous Relar port, CPython, native Codon, and Rust.
In 972 pairs, each side has a single diagnostic. Relar compares the six
fingerprint fields directly in these pairs, avoiding two temporary tuples and
two sorted lists per pair. The [collector profiles and timings](../../benchmarks/results/2026-10-22-collector-ownership.md)
measure the effect across all 1,269 groups.

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

`original_live.py` runs the unchanged upstream discovery, diagnostic conversion,
grouping, statistics, and rendering functions with a bounded CLI, per-child
timeout, typed configuration checks, and atomic publication. It is an upstream
adapter, not the unchanged upstream CLI. The live validator checks complete
reports, exception categories, child reaping, existing-output preservation, and
filesystem effects across upstream Python, typed Python, Relar, Codon, and Rust.

The native Codon build uses explicit TOML, subprocess, and temporary-directory
adapters backed by the independent Rust library in `benchmarks/native_platform`.
That library contains no report computation and links neither Python nor Relar.
Its calls and serialization costs are included in live timings; the Rust
reference uses the same platform library. The declared adapters accommodate
Codon's context-manager behavior and native error representation while keeping
the application function bodies shared.
