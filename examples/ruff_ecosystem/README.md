# Ruff ecosystem reporter

The ten vendored Python modules are the original Ruff ecosystem package. One native executable compares two Ruff binaries on its 58 default Git repositories. Both `check` and `format` use Ruff's own argument parser, temporary configuration changes, Git operations, and Markdown renderers. The source revision and file hashes are recorded in [UPSTREAM.json](UPSTREAM.json).

## Run the original commands

From an Imre checkout, with Git and both Ruff binaries available:

```sh
imre build --entry-profile examples/ruff_ecosystem/markdown-entry.json \
  examples/ruff_ecosystem/ruff_ecosystem/__main__.py -o /tmp/ruff-ecosystem
mkdir -p /tmp/ruff-checkouts
/tmp/ruff-ecosystem --cache /tmp/ruff-checkouts check \
  /path/to/ruff-0.15.0 /path/to/ruff-0.16.1 > check.md
/tmp/ruff-ecosystem --cache /tmp/ruff-checkouts format \
  /path/to/ruff-0.15.0 /path/to/ruff-0.16.1 > format.md
```

Use a cache dedicated to this tool: Ruff resets its clones, and the formatter writes and commits files. Omitting `--cache` gives the run a temporary workspace. The profile supports Markdown, including `--force-preview` and `-v`. The format command uses Ruff's default `ruff-and-ruff`: commit the baseline formatter's changes, reset to the original revision, run the comparison formatter, then render the Git patch. JSON, interactive debugging, and the other formatter comparison modes are outside this profile.

For a comparison against CPython, use a Python 3.12 environment containing `tomli`, `tomli-w`, and `unidiff`. The harness makes disposable copies of the cached repositories with fixed local Git remotes. It times the complete CLI from process launch through Markdown output, compares every byte, and records every repository revision. On Linux, select an available CPU with `--cpu` to make Ruff’s file traversal deterministic. When two configuration files are invalid, the parallel walker can report either error first.

```sh
python3 benchmarks/ruff_ecosystem_live.py --python /path/to/python3.12 \
  --imre /tmp/ruff-ecosystem --old-ruff /path/to/ruff-0.15.0 \
  --new-ruff /path/to/ruff-0.16.1 --cache /tmp/ruff-checkouts \
  --output-dir /tmp/ruff-live-comparison --cpu 3 --rounds 3
```

Pass `--command format` to run the same comparison on the formatter. Add `--capture-format /tmp/ruff-format-patches` to save exactly the 58 patches and errors used in the first CPython report, including the original project options and revisions. Capture uses CPython 3.12 or newer; the timing of that CPython run includes writing the captures.

## Replay frozen diagnostics

`replay.py` reads frozen output from two Ruff versions and passes the sorted diagnostics to Ruff’s original ecosystem package. Captures stay outside Git. Each tab-separated row of `projects.tsv` gives the repository owner, name, commit, and the two capture filenames. A second CLI argument repeats each project under distinct names.

```sh
imre build --reachable-only examples/ruff_ecosystem/replay.py -o /tmp/ruff-replay
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

After compiling the replay with Imre, run the four-engine comparison on Linux. Use CPython 3.12 with Ruff’s ecosystem dependencies and native Codon 0.20.4. The harness measures process wall time and peak resident memory, including input reads, sorting, alignment, and Markdown output. It validates the complete output bytes every time. The 10× workload re-reads the same four files for 20 separately named project comparisons.

```sh
python3 benchmarks/ruff_ecosystem_benchmark.py bench \
  --captures captures --python /path/to/python3.12 --imre /path/to/ruff-replay \
  --codon /tmp/ruff-replay-codon --rust /path/to/ruff-ecosystem-reference \
  --cpu 3 --rounds 4 --output /tmp/ruff-ecosystem-results.json.gz
```

The Rust reference implements the report independently. The Codon reference compiles Ruff’s Markdown function bodies with native typed adapters. Both references exploit the reporter’s stable sort of changed lines. Equal nonpopular runs in sorted inputs directly pair. Contested repeats use the CPython 3.12 alignment rules before sorting. The harness also checks unchanged projects, fix-only changes, non-ASCII paths, popular duplicate rows, and lines that the diagnostic parser skips.

### Replay a preview comparison

Ruff 0.15.0 prints rule codes in preview diagnostics and 0.16.1 prints rule names. The Bokeh comparison changes more than 76,000 lines on each side. The capture command clones the pinned revision from a local checkout, applies Ruff’s original temporary config changes, and verifies both stdout hashes. It also supports Superset and Zulip. [Pinned projects and capture hashes](../../benchmarks/fixtures/ruff_ecosystem/preview.json)

```sh
python3 benchmarks/ruff_ecosystem_preview.py --project bokeh \
  --checkout /path/to/bokeh --old-ruff /path/to/ruff-0.15.0 \
  --new-ruff /path/to/ruff-0.16.1 --output /tmp/bokeh-preview --first 1024
/tmp/ruff-replay /tmp/bokeh-preview 1 > bokeh.md
python3.14 examples/ruff_ecosystem/replay.py /tmp/bokeh-preview 1 > bokeh-cpython.md
cmp bokeh.md bokeh-cpython.md
```

`/tmp/bokeh-preview/first-1024` contains the first 1,024 sorted diagnostics per side for a shorter CPython 3.12 comparison. CPython 3.14 bounds its search for intraline matches. The frozen reports use the replay driver’s standard project options. To compare the full 58-project CLI with its original command lines and project errors, use the live benchmark command above with `--force-preview --python /path/to/python3.14`.

For the three-project timing comparison, write the captures under `preview/bokeh`, `preview/superset`, and `preview/zulip`. The harness verifies the input and Markdown hashes, runs each native executable once to warm up, and rotates the three executables through six measured rounds on one CPU. Passing `--python /path/to/python3.14` also times the original script once per complete project and compares the output bytes directly.

```sh
python3 benchmarks/ruff_ecosystem_benchmark.py preview --captures preview \
  --imre /tmp/ruff-replay --codon /tmp/ruff-replay-codon \
  --rust /path/to/ruff-ecosystem-reference --cpu 3 --output preview-results.json.gz
```

### Replay all 58 formatter patches

`format_replay.py` passes Git patches and formatter errors to Ruff’s original `markdown_format_result`. Capture them from the **same** CPython invocation that prints the expected Markdown. This matters because Ruff’s parallel walker can find either of two invalid Airflow configuration files first; its error messages also include the absolute checkout path.

Use disposable checkouts with local Git remotes at the [pinned revisions](../../benchmarks/fixtures/ruff_ecosystem/format-projects.json), such as the `checkouts/` directory created by the live harness above. Run these commands from the Imre repository. The Python environment needs `tomli`, `tomli-w`, and `unidiff==1.0.0`; the capture hook uses CPython 3.12 or newer.

```sh
python3.14 benchmarks/ruff_ecosystem_format_capture.py /tmp/format-captures \
  benchmarks/fixtures/ruff_ecosystem/format-projects.json \
  --cache /path/to/disposable/checkouts format /path/to/ruff-0.15.0 /path/to/ruff-0.16.1 \
  > /tmp/format-expected.md
imre build --reachable-only examples/ruff_ecosystem/format_replay.py -o /tmp/format-imre
/tmp/format-imre /tmp/format-captures 1 > /tmp/format-imre.md
cmp /tmp/format-expected.md /tmp/format-imre.md
```

Each row of `projects.tsv` specifies the owner, project, revision, preview flag, exclude pattern, outcome (`ok` or `error`), and input filename. Tabs separate the fields. At 10×, the replay reads every patch ten times under distinct project names and renders one 580-project report.

```sh
cargo build --release --locked --manifest-path benchmarks/reference/ruff_ecosystem/Cargo.toml --bin format
python3 benchmarks/codon_native/ruff_ecosystem_format_build.py --codon /path/to/codon --output /tmp/format-codon
python3 benchmarks/ruff_ecosystem_format.py --captures /tmp/format-captures \
  --python /path/to/python3.14 --imre /tmp/format-imre --codon /tmp/format-codon \
  --rust benchmarks/reference/ruff_ecosystem/target/release/format --cpu 3 --output /tmp/format-times.json
```

The Rust reference parses the Git patches and writes Markdown independently. Codon compiles Ruff’s Markdown function bodies with native patch types. The Linux harness checks each Imre project report separately against CPython, compares every combined report byte-for-byte, and rotates all four processes across eight measured rounds on one CPU. Wall time includes startup, patch reads, rendering, and writing the full Markdown. Git and Ruff subprocesses run only during capture.

On the pinned revisions, the 58-project report contained 693,937 bytes of Markdown: 22 changed projects, 30 unchanged projects, and six formatter errors. The 580-project report contained 7,014,958 bytes.

| Frozen input | CPython 3.14 | Imre | Codon 0.20.4 | Rust | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: |
| 58 projects | 174.8 ms | 12.0 ms | 31.5 ms | 8.7 ms | 1.37× |
| 580 projects | 749.9 ms | 107.0 ms | 233.1 ms | 62.9 ms | 1.70× |

At 10×, median peak RSS was 37.8 MiB for Imre, 16.5 MiB for Rust, 88.3 MiB for Codon, and 107.1 MiB for CPython. Across the 58-project replay, the input and output optimizations cut heap allocations from 85,005 to 25,032 (Valgrind DHAT). The original Imre renderer allocated an owned string for each hunk line before joining it again. The compiler now groups the tail of a split when it can prove that the only use is a private list’s final join. Enabling just this optimization on the same generated LLVM program brought the 10× replay from 131.5 to 103.3 ms in a separate paired run. [Input hashes, individual report digests, peak memory, and raw timings](../../benchmarks/results/2026-10-24-ruff-ecosystem-format.json.gz)
