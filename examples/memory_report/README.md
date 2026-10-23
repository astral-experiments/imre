# Compare ty memory reports

## Run Ruff’s original script

From a Ruff checkout, compare a pair of existing captures:

```sh
relar run scripts/memory_report.py -- compare --old old.json --new new.json
```

Use `--old-dir` and `--new-dir` for directories of captures. Add
`--output report.md` to write the Markdown to a file.

```sh
RELAR_PYTHON="$(command -v python3)" relar run scripts/memory_report.py -- run \
  --old-ty /path/to/old/ty --new-ty /path/to/new/ty \
  --projects-dir ./memory-projects --old-reports-dir ./old --new-reports-dir ./new
```

The native reporter runs Ruff’s neighboring `setup_primer_project.py` through
its locked uv environment. The first run clones four projects and installs their
dependencies. Subsequent runs reuse those projects and collect eight fresh ty
reports. Keep the Ruff checkout available to a built executable that uses
`run`. The `compare` command can run independently.

The [comparison benchmarks](../../benchmarks/results/2026-10-23-original-memory-report.md)
cover 1× and 10× input against CPython, Codon, and Rust. The
[live runs](../../benchmarks/results/2026-10-23-original-memory-live.md) time the
original command on all four projects with both ty executables.

## Run the typed port

This is a typed port of Ruff's real
[`scripts/memory_report.py`](https://github.com/astral-sh/ruff/blob/3e4fffb05a2b143fafbf31d891c79c878fe3bcd3/scripts/memory_report.py)
file and directory comparison modes, plus a prepared-project collection mode.
It reads `TY_MEMORY_REPORT=json` captures and
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

Run two existing ty executables over project trees that are already prepared:

```sh
cat >/tmp/projects.json <<'JSON'
{"projects":[{"name":"inventory","cwd":".","argv":["{ty}","check","--python-version","3.14","--extra-search-path","examples","examples/inventory_reconciliation"]}]}
JSON
/tmp/memory-report run --projects /tmp/projects.json \
  --old-ty /absolute/path/to/old/ty --new-ty /absolute/path/to/new/ty \
  --output /tmp/memory-report.md
```

Each manifest entry has a unique nonempty `name`, a nonempty `cwd`, and a string
`argv` list containing `{ty}`. Relative working directories are relative to the
report program's working directory. Every exact `{ty}` argument is replaced with
the selected absolute executable path. Arguments are passed directly without a
shell. Types, nonempty fields, unique names, and placeholders are validated for
the complete manifest before any child starts or capture directory is created.
Operating-system checks, such as invalid paths or arguments, happen at launch.
Projects run
in manifest order, old then new; the report uses deterministic project-name order.
Children inherit a copy of the environment with `TY_MEMORY_REPORT=json` and
`TY_MAX_PARALLELISM=1`. Both output streams are captured as strict UTF-8 text, and
nonzero child status raises `CalledProcessError`. Only stdout is decoded as a
memory report; successful diagnostic stderr is discarded unless captures are
requested.

Add `--capture-dir` to retain each child's report and diagnostics:

```sh
/tmp/memory-report run --projects /tmp/projects.json \
  --old-ty /absolute/path/to/old/ty --new-ty /absolute/path/to/new/ty \
  --capture-dir /tmp/memory-captures --output /tmp/memory-report.md
```

The capture directory must be new and its parent must already exist. It contains
`projects.json` and a `project-0`, `project-1`, … directory for each started
project, in manifest order. Each project directory can contain `old.stdout.txt`,
`old.stderr.txt`, `new.stdout.txt`, and `new.stderr.txt`. Numeric directory names
keep arbitrary project names out of filesystem paths. The manifest and streams
are UTF-8 text with universal newlines normalized to `\n`.

Captures intentionally preserve a partial transcript on failure. Each stream is
staged, closed, and individually replaced before checking the child's exit
status or decoding its JSON. A failed child or malformed JSON therefore retains
its decoded streams. A launch or UTF-8 decoding failure produces no transcript
for that invocation, and no later child runs. The two files are published
separately: an error replacing stderr can leave stdout published. Existing
capture directories are never reused. When `--capture-dir` is supplied, the
output basename may not be `projects.json`, `old.stdout.txt`, `old.stderr.txt`,
`new.stdout.txt`, or `new.stderr.txt`, even outside the capture directory.

All modes finish decoding, comparison, and rendering before staging the final
Markdown in a private temporary directory beside the destination. A successful
write and close is followed by an atomic replacement of the destination; that
replacement is the commit point. Earlier failures preserve an existing report.
Cleanup failures propagate, including errors after the new report has already
been committed. Publication creates a new file entry and inode with permissions
subject to the process umask. Replacing an output symlink or hardlink leaves its
previous target's contents unchanged. No fsync or crash-durability guarantee is
provided. Child side effects and published captures are not rolled back. Project
checkout, dependency installation, and ty installation remain preparation steps.

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

File and directory comparison accept optional `compare` before the flags.
The prepared-project `run` command reuses the same comparison and renderer.
Successful file output is quiet; the upstream script
also echoes the completed report to stderr.
CLI validation errors use `ValueError`; upstream prints its own diagnostics.

Validate reports, strict typed failures, and complete filesystem effects:

```sh
python3 benchmarks/reference/memory_report/build.py \
  --artifact-dir /tmp/memory-rust
python3 benchmarks/memory_validate.py --binary /tmp/memory-report \
  --rust /tmp/memory-rust/memory-report-reference
```

The [benchmark guide](../../benchmarks/reference/memory_report/README.md) compares
Relar, independent Rust, typed CPython, and CPython adapters using upstream's
unchanged models and renderer. The comparison adapter adds atomic output to its
existing CLI; the prepared-project adapter adds the collection and capture
workflow. Workloads include real captures, actual ty child runs, and clearly
labeled larger copies.

The [native-path results](../../benchmarks/results/2026-10-21-native-paths.md)
take 0.91–1.28× Rust's time for complete directory comparisons and
1.38–1.63× for repeated parsing, comparison, and rendering. Both layouts contain
65,024 items per side and meet the ≤2× target in ordinary and ThinLTO builds.
Complete directory runs are 2.13–2.89× faster than the unchanged upstream
CPython script. The report includes validation and replayable benchmark evidence.

The [workspace and atomic-output results](../../benchmarks/results/2026-10-21-native-workspaces.md)
cover actual old/new ty launches and synthetic workloads with 16,256 items per
side, with and without retained transcripts. Complete process runs take
1.00–1.57× Rust's time, file-only reports take 1.13–1.26×, and repeated report
computation takes 1.42–1.57×, across ordinary and ThinLTO builds. All 26 workload
rows meet the ≤2× target, including controls. The report retains all 3,744
measured executions and replayable validation evidence.
