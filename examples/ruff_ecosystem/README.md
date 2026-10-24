# Ruff ecosystem checker

The ten vendored Python modules are the original Ruff ecosystem package. The native executable checks two Ruff binaries against its 58 default Git repositories, using the same argument parser, temporary config changes and Markdown renderer as CPython. The source revision and file hashes are recorded in [UPSTREAM.json](UPSTREAM.json).

## Run the original check command

From a Relar checkout, with Git and both Ruff binaries available:

```sh
relar build --entry-profile examples/ruff_ecosystem/check-entry.json \
  examples/ruff_ecosystem/ruff_ecosystem/__main__.py -o /tmp/ruff-ecosystem
mkdir -p /tmp/ruff-checkouts
/tmp/ruff-ecosystem --cache /tmp/ruff-checkouts check \
  /path/to/ruff-0.15.0 /path/to/ruff-0.16.1 > report.md
```

The first run clones the repositories. Subsequent runs reset and pull those checkouts. Omitting `--cache` gives the run a temporary workspace. The included entry profile selects `check` and Markdown output. `--force-preview` and `-v` work with that profile. Selecting the format command, JSON output, or the interactive debugger raises an error naming the required mode.

For a comparison against CPython, use a Python 3.12 environment containing `tomli`, `tomli-w`, and `unidiff`. The harness makes disposable copies of the cached repositories with fixed local Git remotes. It times the complete CLI from process launch through Markdown output, compares every byte, and records every repository revision. On Linux, select an available CPU with `--cpu` to make Ruff’s file traversal deterministic. When two configuration files are invalid, the parallel walker can report either error first.

```sh
python3 benchmarks/ruff_ecosystem_live.py --python /path/to/python3.12 \
  --relar /tmp/ruff-ecosystem --old-ruff /path/to/ruff-0.15.0 \
  --new-ruff /path/to/ruff-0.16.1 --cache /tmp/ruff-checkouts \
  --output-dir /tmp/ruff-live-comparison --cpu 3 --rounds 3
```

## Replay frozen diagnostics

`replay.py` reads frozen output from two Ruff versions and passes the sorted diagnostics to Ruff’s original ecosystem package. Captures stay outside Git. Each tab-separated row of `projects.tsv` gives the repository owner, name, commit, and the two capture filenames. A second CLI argument repeats each project under distinct names.

```sh
relar build --reachable-only examples/ruff_ecosystem/replay.py -o /tmp/ruff-replay
/tmp/ruff-replay captures 1 > report.md
python3 examples/ruff_ecosystem/replay.py captures 1 > cpython-report.md
cmp report.md cpython-report.md
```

The benchmark corpus uses Ruff 0.15.0 and 0.16.1 on full Sphinx and Prefect checkouts. Check out the revisions in the [fixture manifest](../../benchmarks/fixtures/ruff_ecosystem/corpus.json), with directories named `sphinx` and `prefect`. The capture command checks both Ruff versions, repository revisions, and the four stdout hashes.

```sh
python3 benchmarks/ruff_ecosystem_benchmark.py capture \
  --checkouts /path/to/projects --old-ruff /path/to/ruff-0.15.0 \
  --new-ruff /path/to/ruff-0.16.1 --output captures
cargo build --release --locked --manifest-path benchmarks/reference/ruff_ecosystem/Cargo.toml
python3 benchmarks/codon_native/ruff_ecosystem_build.py --codon /path/to/codon --output /tmp/ruff-replay-codon
```

After compiling the replay with Relar, run the four-engine comparison on Linux. Use CPython 3.12 with Ruff’s ecosystem dependencies and native Codon 0.20.4. The harness measures process wall time and peak resident memory, including input reads, sorting, alignment, and Markdown output. It validates the complete output bytes every time. The 10× workload re-reads the same four files for 20 separately named project comparisons.

```sh
python3 benchmarks/ruff_ecosystem_benchmark.py bench \
  --captures captures --python /path/to/python3.12 --relar /path/to/ruff-replay \
  --codon /tmp/ruff-replay-codon --rust /path/to/ruff-ecosystem-reference \
  --cpu 3 --rounds 4 --output /tmp/ruff-ecosystem-results.json.gz
```

The Rust reference implements the report independently. The Codon reference compiles Ruff’s Markdown function bodies with native typed adapters. Both references exploit the reporter’s stable sort of changed lines. Equal nonpopular runs in sorted inputs directly pair. Contested repeats use the CPython 3.12 alignment rules before sorting. The harness also checks unchanged projects, fix-only changes, non-ASCII paths, popular duplicate rows, and lines that the diagnostic parser skips.
