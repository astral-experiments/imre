# Compare ty memory reports

This is a typed port of Ruff's real
[`scripts/memory_report.py`](https://github.com/astral-sh/ruff/blob/3e4fffb05a2b143fafbf31d891c79c878fe3bcd3/scripts/memory_report.py)
file and directory comparison modes. It reads `TY_MEMORY_REPORT=json` captures and
produces the same Markdown summary and largest-change tables. The retained
upstream source and license are in [upstream/](upstream/).

From the repository root, after building Relar:

```sh
target/release/relar build examples/memory_report/__main__.py \
  --module-root examples --clang clang-18 -o /tmp/memory-report
/tmp/memory-report \
  --old examples/memory_report/sample/ruff-scripts-old.json \
  --new examples/memory_report/sample/ruff-scripts.json \
  --output /tmp/memory-report.md
cat /tmp/memory-report.md
```

Omit `--output` to print the report. The project name comes from the new file's
stem. The sample compares ty 0.0.89 and 0.0.90 checking Ruff's actual `scripts`
tree: reported memory falls from 44,380,774 to 44,342,862 bytes. A second genuine
capture pair covers this repository's inventory reconciler. Exact captures,
producer versions, commands, binary/source hashes, and producer warning logs are
retained in [sample/](sample/provenance.json).

Compare matching project reports in two directories:

```sh
/tmp/memory-report compare --old-dir old-reports --new-dir new-reports \
  --output /tmp/memory-report.md
```

Each directory is scanned once for `*.json`, including dotfiles. The file stem
identifies a project; only project names present in both directories appear in
the report. Every selected file is decoded, including unmatched projects, and
common names are sorted before stable ranking by total change. Nested files and
other extensions are ignored. A matching directory is an input error. Empty,
missing, or disjoint report directories produce a clear error and preserve any
existing output.

Run the same typed source on CPython with the compatibility library:

```sh
PYTHONPATH=stdlib:examples python3 -m memory_report \
  --old examples/memory_report/sample/ruff-scripts-old.json \
  --new examples/memory_report/sample/ruff-scripts.json
```

The models use `@dataclass(slots=True)`, scalar defaults, keyword-only fields,
and `field(default_factory=list)`. `ProjectComparison.total_diff_bytes` is a
read-only property, and changed-item rows are frozen dataclasses. These compile
to Relar's fixed-layout records and direct function calls.

`load.py` uses native `pathlib.Path` values, `glob`, and `Path.stem`, then calls
`MemoryReport.from_json(path)`. That class method reads UTF-8 text and uses
`relar.json.decode(text, cls)` and returns `Self`. The compiler generates a
decoder for concrete record fields; there is no exposed dynamic JSON object.
The CPython implementation in `stdlib/relar/json.py` enforces the same boundary.
Integers retain arbitrary precision. Required fields must be present, item byte
counts default to zero, and absent report lists become fresh empty lists.
Explicit `null` does not satisfy an exact integer, string, or list field.
Duplicate known fields are rejected; unknown fields are syntax-checked and
ignored. JSON nesting is limited to 128 object/array levels.

The port preserves the upstream 10,000-byte significance threshold, binary size
units, percentage formatting, last-value-wins duplicate item names, space removal
in displayed item names, and the largest 15 changed items. Upstream builds a set
before sorting, so equal-change order and selection can vary with the Python
hash seed. This port uses stable first appearance: old items first, then new-only
items. The independent validator permits only that upstream tie ambiguity.

This example implements file and directory comparison, with optional `compare`
before the flags. Upstream's project checkout and ty execution modes remain
outside its scope. Successful file output is quiet; the upstream script
also echoes the completed report to stderr. Parsing and formatting finish before
the output file opens, preserving an existing report on those failures.
CLI validation errors use `ValueError`; upstream prints its own diagnostics.

Validate reports, strict typed failures, and complete filesystem effects:

```sh
python3 benchmarks/reference/memory_report/build.py \
  --artifact-dir /tmp/memory-rust
python3 benchmarks/memory_validate.py --binary /tmp/memory-report \
  --rust /tmp/memory-rust/memory-report-reference
```

The [benchmark guide](../../benchmarks/reference/memory_report/README.md) compares
the unchanged upstream script, typed CPython port, Relar, and independent Rust on
real captures and clearly labeled larger copies.

The [dataclass results](../../benchmarks/results/2026-10-21-dataclass-records.md)
take 1.16–1.47× Rust's time on 65,024 items per input file, with the same report
semantics in ordinary and ThinLTO builds. The report includes the original
CPython comparison, full validation, and replayable benchmark evidence.
