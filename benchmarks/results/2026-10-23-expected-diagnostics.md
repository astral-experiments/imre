# Ruff’s expected diagnostic collector

Relar compiles all 37 lines of Ruff’s `collect_expected_diagnostics` unchanged. The live reporter discovers paths, collects the annotations before either ty invocation, and renders from those records. The Rust reference also scans once. Frozen replay reads each source and both JSON captures once, then scans the buffered text on each compute round. [Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L469-L505)

## Live reporter

Wall times are medians in milliseconds from eight runs per engine after one warmup, with engine order balanced across runs. The changed GitHub report includes the forced summary table. These are ordinary optimized Linux x86-64 builds on an AMD EPYC-Milan host. Relar and Rust both use jemalloc. The comparison uses CPython 3.14.5 and native Codon 0.20.4 with its existing JSON, regex and OS adapters.

| Workload | Ruff on CPython | Codon | Relar | Rust | Relar / Rust |
| --- | ---: | ---: | ---: | ---: | ---: |
| 146 files, two real ty runs | 750.8 | 791.4 | 668.3 | 666.9 | 1.002× |
| 146 files, replayed ty output | 249.8 | 269.2 | 185.9 | 172.7 | 1.077× |
| 1,460 files, replayed ty output | 1,615.3 | 2,701.8 | 1,790.3 | 1,643.7 | 1.089× |

The real checkout uses ty 0.0.60 and 0.0.90 on two CPU cores. To measure the same live reporter with analyzer computation removed, the replay launches two child processes that stream the captured JSON. Every engine still discovers and reads the source files, collects expected diagnostics, resolves diagnostic paths, and publishes the report. At 1,460 files, Ruff on CPython completed this replay faster than both native Rust and Relar. All four engines published the same 109,258 bytes at 1× and 1,101,967 bytes at 10×.

## Frozen reports

Each fresh process reads the manifest, sources and two captures, then writes a complete report. The 10× fixture contains ten copies of the suite with distinct filenames. Each value is the median of 12 runs following one warmup, in balanced order on one CPU per build configuration. Ordinary ran on CPU 8 and ThinLTO on CPU 9. The previous Relar control is the merged #115 compiler and reporter, linked against the same runtime archive as the new reporter. Native Codon uses the matching build of its platform library.

| Build | Files | Report | Ruff on CPython | Typed CPython | Previous Relar | Codon | Relar | Rust | Relar / Rust |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | 146 | changed GitHub | 172.4 | 271.8 | 39.4 | 75.5 | 39.7 | 27.7 | 1.44× |
| Ordinary | 146 | changed diff | 167.8 | 270.4 | 38.5 | 74.0 | 38.4 | 26.3 | 1.46× |
| Ordinary | 146 | unchanged GitHub | 171.5 | 278.7 | 37.6 | 72.8 | 37.5 | 25.9 | 1.45× |
| Ordinary | 146 | unchanged diff | 176.2 | 280.0 | 38.5 | 73.2 | 38.6 | 26.4 | 1.46× |
| Ordinary | 1,460 | changed GitHub | 990.8 | 2,047.1 | 341.3 | 685.7 | 342.4 | 246.4 | 1.39× |
| Ordinary | 1,460 | changed diff | 995.5 | 2,037.4 | 339.9 | 687.3 | 340.2 | 245.7 | 1.38× |
| Ordinary | 1,460 | unchanged GitHub | 969.9 | 2,024.5 | 333.6 | 650.2 | 327.5 | 241.9 | 1.35× |
| Ordinary | 1,460 | unchanged diff | 958.6 | 2,028.9 | 333.7 | 647.2 | 328.9 | 239.3 | 1.37× |
| ThinLTO | 146 | changed GitHub | 172.2 | 272.9 | 36.1 | 75.4 | 36.3 | 26.1 | 1.39× |
| ThinLTO | 146 | changed diff | 171.6 | 273.6 | 35.8 | 74.7 | 35.1 | 25.3 | 1.39× |
| ThinLTO | 146 | unchanged GitHub | 172.8 | 278.8 | 34.0 | 72.6 | 35.1 | 24.5 | 1.43× |
| ThinLTO | 146 | unchanged diff | 179.7 | 285.2 | 36.8 | 74.7 | 35.9 | 26.8 | 1.34× |
| ThinLTO | 1,460 | changed GitHub | 983.7 | 2,009.2 | 300.7 | 681.6 | 303.4 | 230.6 | 1.32× |
| ThinLTO | 1,460 | changed diff | 958.9 | 1,974.8 | 298.7 | 676.0 | 297.6 | 227.8 | 1.31× |
| ThinLTO | 1,460 | unchanged GitHub | 955.0 | 1,999.1 | 293.8 | 643.8 | 294.2 | 223.7 | 1.31× |
| ThinLTO | 1,460 | unchanged diff | 928.5 | 1,954.8 | 290.1 | 637.5 | 290.0 | 220.6 | 1.31× |

Every timed output matched the pinned upstream byte for byte in both builds. Frozen reports took 1.31–1.46× Rust’s time.

## Default system allocator

Ordinary builds with the shipped system allocator also passed the 2× target against system-allocator Rust. The table gives medians in milliseconds over eight paired live runs. Each run alternated which engine went first. All output hashes matched the four-engine comparison.

| Live workload | Relar | Rust | Relar / Rust |
| --- | ---: | ---: | ---: |
| 146 files, two real ty runs | 660.9 | 649.1 | 1.018× |
| 146 files, replayed ty output | 191.9 | 172.9 | 1.110× |
| 1,460 files, replayed ty output | 1,815.7 | 1,646.5 | 1.103× |

The eight frozen cases took 1.44–1.53× Rust’s time over 12 paired runs each.

## Source and input checks

The native collector matched every ordered field of 1,323 expected diagnostics at 1× and 13,230 at 10×. A Unicode and mixed-newline fixture also matched, including character columns, named tags, optional markers and multi-line tags. A live test overwrites a test file with invalid bytes while ty runs. The report retains the original annotations. Invalid UTF-8 or a suite with no annotations fails before ty starts.

A process-scoped file-open trace counted exactly one open for each of the 149 frozen inputs at 1× and 1,463 at 10×, at zero, one and three compute rounds. The 400 seeded comparisons also render after the entire input directory is moved. All 100 five-engine live checks and 367 frozen process checks passed. Live replays at both sizes also produced identical output under AddressSanitizer with leak detection enabled.

## Reproduce

Materialize both fixture scales with `benchmarks/conformance_grouping_inputs.py`, then run the captured-report driver with `--reports-only`. The live measurement uses the example’s checkout command. For live replay, place the materialized test files under the checkout’s `tests/` directory, declare their filename prefixes in `src/test_groups.toml`, resolve each captured diagnostic through the manifest to `tests/<filename>`, and pass a child that emits each capture in place of ty. The compressed measurement record includes the replay driver, binary and source identities, inputs, output digests, build flags, file-open probe, and every raw sample. [Example and fixture instructions](../../examples/conformance_report/README.md) · [Captured-report driver](../conformance_grouping_benchmark.py) · [Measurement record](2026-10-23-expected-diagnostics.json.gz)
