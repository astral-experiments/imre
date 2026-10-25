> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff diagnostic grouping and per-file statistics

Imre compiles Ruff’s three original analysis functions, a total of 2,768 bytes of Python. The real changed run produced 1,269 ordered groups from 146 typing-conformance files. The larger run produced 12,690 groups from 1,460 distinct files. All six implementations matched the fields of every grouped record, both sets of per-file statistics, and every byte of both report formats. [Pinned Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L549) · [Imre application](../../examples/conformance_report/model.py)

With the default system allocator, Imre took 1.20–1.83× Rust’s time for grouping and 1.30–1.38× for complete report invocations. The largest median on the canonical checkout path across the 32 ordinary and ThinLTO workloads was 1.83× Rust.

A separate native-only control reached 2.28× Rust for 10× grouping on a short checkout path and 2.13× on the canonical path with ThinLTO. With `--allocator jemalloc`, the same grouping code took 1.07–1.17× Rust’s time across both paths and link modes when Rust also used jemalloc. [Linux allocator comparison](2026-10-23-diagnostic-allocator.md)

## Isolated analysis

Milliseconds per pass over preloaded diagnostic records. Grouping starts from parsed old, new and expected records. Equivalence and file statistics start from already-grouped cases. Every file-statistics pass resolves the full file list before collecting counts.

| Corpus | Work | Link | Ruff CPython | Typed CPython | Previous Imre | Imre | Codon | Rust | Imre/Rust |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1× | Group diagnostics | Ordinary | 3.891 | 3.482 | 5.865 | 2.218 | 6.803 | 1.758 | 1.26× |
| 1× | Group diagnostics | ThinLTO | 3.843 | 3.518 | 4.638 | 1.793 | 6.840 | 1.497 | 1.20× |
| 1× | Compare fingerprints | Ordinary | 1.457 | 1.452 | 0.127 | 0.115 | 1.751 | 0.092 | 1.25× |
| 1× | Compare fingerprints | ThinLTO | 1.439 | 1.450 | 0.078 | 0.071 | 1.740 | 0.062 | 1.15× |
| 1× | Collect file statistics | Ordinary | 9.249 | 9.006 | 0.269 | 4.540 | 6.241 | 2.924 | 1.55× |
| 1× | Collect file statistics | ThinLTO | 9.224 | 9.114 | 0.229 | 4.365 | 6.248 | 2.867 | 1.52× |
| 10× | Group diagnostics | Ordinary | 73.211 | 53.707 | 83.312 | 37.052 | 88.171 | 20.286 | 1.83× |
| 10× | Group diagnostics | ThinLTO | 72.333 | 54.867 | 69.975 | 31.866 | 86.465 | 17.833 | 1.79× |
| 10× | Compare fingerprints | Ordinary | 17.764 | 16.117 | 1.527 | 1.382 | 20.847 | 1.155 | 1.20× |
| 10× | Compare fingerprints | ThinLTO | 17.669 | 16.147 | 1.020 | 0.919 | 20.616 | 0.786 | 1.17× |
| 10× | Collect file statistics | Ordinary | 104.72 | 100.10 | 3.706 | 46.450 | 65.361 | 30.958 | 1.50× |
| 10× | Collect file statistics | ThinLTO | 100.32 | 98.053 | 2.917 | 45.185 | 65.972 | 29.711 | 1.52× |

The previous string-based port skipped filesystem resolution on each statistics pass. Ruff resolves every path and merges aliases that name the same file. The differential checks include a symlink that produced a fifth file row on the previous port and four rows in Ruff, current Imre, and Rust. The independent Rust reference calls the OS to resolve each input path, preserves input order, and handles missing paths through its non-strict resolver. [Previous port](https://github.com/astral-sh/imre/blob/17845276508a0431276b2606f4f076dcd58ecf37/examples/conformance_report/model.py) · [Rust reference](../reference/conformance_report/src/main.rs)

A separate 12-pair run on a second Linux devbox compiled the previous string port with the same final compiler and runtime. Switching that build to Ruff’s source sped up grouping 1.89× at 1× (Ordinary), 1.65× at 10× (Ordinary), 1.81× at 1× (ThinLTO), 1.60× at 10× (ThinLTO). The complete group and file-statistic dumps matched. On that second host, 10× grouping took 2.13× Rust’s time in both link modes.

### Checkout-path length

Grouping sorts string keys that contain the absolute source path. A separate control on the same pinned CPU varied only the root of the checkout. Each row used eight counterbalanced pairs and identical files, diagnostics, and normalized group transcripts.

| Link | Files | Common directory prefix (bytes) | Imre (ms) | Rust (ms) | Imre/Rust |
|---|---:|---:|---:|---:|---:|
| Ordinary | 146 | 70 | 2.175 | 1.747 | 1.24× |
| Ordinary | 146 | 23 | 2.135 | 1.591 | 1.34× |
| Ordinary | 1,460 | 71 | 38.353 | 19.761 | 1.94× |
| Ordinary | 1,460 | 24 | 36.658 | 17.810 | 2.06× |
| ThinLTO | 146 | 70 | 1.787 | 1.493 | 1.20× |
| ThinLTO | 146 | 23 | 1.736 | 1.331 | 1.30× |
| ThinLTO | 1,460 | 71 | 37.334 | 17.538 | 2.13× |
| ThinLTO | 1,460 | 24 | 35.096 | 15.390 | 2.28× |

The native-only 10× control exceeded 2× Rust on the short path in both modes and on the canonical path with ThinLTO. Both engines ran faster under the short path; Rust gained more. The control also used 12 repeated passes per process, versus eight in the six-engine panel. The 2× result for isolated 10× grouping therefore depends on the checkout and measurement protocol.

## Complete reports

Milliseconds. A complete invocation reads the manifest, individual source files and two captured GitLab JSON arrays, computes the report, and writes its Markdown. The repeated panel preloads raw input, then reruns annotation scanning, JSON decoding, analysis, and rendering for each pass.

| Corpus | Work | Link | Ruff CPython | Typed CPython | Previous Imre | Imre | Codon | Rust | Imre/Rust |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1× | Changed GitHub | Ordinary | 159.65 | 259.91 | 34.859 | 34.736 | 65.777 | 25.200 | 1.38× |
| 1× | Changed GitHub | ThinLTO | 160.73 | 261.61 | 31.251 | 31.959 | 66.090 | 24.433 | 1.31× |
| 1× | Changed diff | Ordinary | 159.53 | 260.92 | 34.355 | 34.394 | 66.020 | 25.157 | 1.37× |
| 1× | Changed diff | ThinLTO | 159.30 | 260.59 | 30.557 | 31.065 | 65.888 | 23.704 | 1.31× |
| 1× | Unchanged GitHub | Ordinary | 157.32 | 260.90 | 33.639 | 33.503 | 63.591 | 24.704 | 1.36× |
| 1× | Unchanged GitHub | ThinLTO | 157.68 | 261.55 | 29.801 | 30.593 | 63.733 | 23.335 | 1.31× |
| 1× | Unchanged diff | Ordinary | 162.39 | 263.97 | 33.655 | 33.572 | 63.684 | 24.807 | 1.35× |
| 1× | Unchanged diff | ThinLTO | 156.83 | 259.57 | 29.874 | 30.549 | 64.041 | 23.246 | 1.31× |
| 1× | Changed GitHub, repeated | Ordinary | 57.528 | 154.33 | 16.753 | 17.104 | 38.577 | 12.732 | 1.34× |
| 1× | Changed GitHub, repeated | ThinLTO | 56.759 | 153.21 | 13.206 | 14.297 | 38.022 | 11.396 | 1.25× |
| 10× | Changed GitHub | Ordinary | 922.26 | 1,928.12 | 332.49 | 324.06 | 609.44 | 243.20 | 1.33× |
| 10× | Changed GitHub | ThinLTO | 906.57 | 1,917.47 | 285.82 | 291.53 | 607.48 | 223.73 | 1.30× |
| 10× | Changed diff | Ordinary | 901.30 | 1,888.80 | 322.52 | 318.17 | 602.01 | 235.67 | 1.35× |
| 10× | Changed diff | ThinLTO | 916.04 | 1,923.79 | 285.81 | 286.12 | 612.78 | 219.64 | 1.30× |
| 10× | Unchanged GitHub | Ordinary | 876.49 | 1,894.98 | 313.44 | 308.37 | 582.97 | 228.73 | 1.35× |
| 10× | Unchanged GitHub | ThinLTO | 886.71 | 1,895.93 | 274.38 | 277.80 | 587.26 | 212.81 | 1.31× |
| 10× | Unchanged diff | Ordinary | 876.36 | 1,882.79 | 311.27 | 306.54 | 581.88 | 227.71 | 1.35× |
| 10× | Unchanged diff | ThinLTO | 882.29 | 1,909.11 | 276.37 | 279.95 | 588.25 | 213.87 | 1.31× |
| 10× | Changed GitHub, repeated | Ordinary | 695.41 | 1,662.36 | 203.10 | 202.79 | 482.35 | 153.81 | 1.32× |
| 10× | Changed GitHub, repeated | ThinLTO | 689.83 | 1,654.07 | 164.97 | 172.59 | 471.00 | 137.20 | 1.26× |

Complete invocations take 0.97–1.03× the previous port’s time. The full measurements include input decoding, per-file filesystem resolution, and Markdown rendering as well as grouping.

Separate fresh processes measured peak resident memory for the changed GitHub report. Values are the median of six observations in MiB, using Linux `ru_maxrss` from GNU time.

| Corpus | Link | Ruff CPython | Typed CPython | Previous Imre | Imre | Codon | Rust |
|---|---|---:|---:|---:|---:|---:|---:|
| 1× | Ordinary | 27.0 | 23.8 | 9.5 | 9.7 | 19.5 | 7.7 |
| 1× | ThinLTO | 27.0 | 23.8 | 10.2 | 10.5 | 19.5 | 8.2 |
| 10× | Ordinary | 91.7 | 74.1 | 46.7 | 46.2 | 115.0 | 54.7 |
| 10× | ThinLTO | 91.7 | 74.2 | 47.4 | 47.1 | 115.0 | 55.1 |

A separate live checkout run included both ty processes for every changed GitHub report on 146 files. On a second Linux devbox with CPUs 20 and 21 and two Rayon threads, medians were 797.2 ms for upstream CPython, 716.0 ms for ordinary Imre, and 708.2 ms for Rust (1.01× Imre/Rust). All 12 measurements per engine produced the same 109,258 bytes.

## Native profiles

The 10× ordinary kernel ran on a second Linux devbox under Callgrind and DHAT, with the same executable hashes and zero or four rounds. Instruction counts, allocated blocks and cumulative allocated bytes subtract the preload and divide by four. Peak live heap is the observed high-water mark for the entire four-round process.

| Engine | Instructions/pass | Allocations/pass | Allocated MB/pass | Peak live MB |
|---|---:|---:|---:|---:|
| Previous Imre | 474,696,986 | 357,848 | 26.67 | 41.10 |
| Imre | 170,194,324 | 126,670 | 13.47 | 41.50 |
| Rust | 169,523,366 | 172,695 | 21.50 | 33.13 |

Before the sorted-record gather, the ordinary 10× kernel took 41.58 ms, 2.06× Rust, on the canonical checkout. That earlier source and complete timing panel are included under `intermediate/` in the study.

The source uses three list-factory dictionaries followed by a sorted key union. Imre recognizes the fixed record constructed from each key, gathers the three list owners in one traversal, and keeps each result row’s key and buckets together through sorting. The compiler checks the list-factory dictionary types, generated initializer, assignment order, and absence of user callbacks. [Compiler proof](../../crates/imre-compiler/src/frontend/sorted_rows.rs) · [Runtime](../../crates/imre-runtime/src/objects/mappings/sorted_rows.rs)

SipHash's byte-feed function fell from 107.1 million to 21.9 million instructions per 10× pass. The previous port also made 93,120 buffer allocations on one path-normalization stack. In the current port, the largest allocation stack accounts for 34,150 list owners created on first access to a grouping key. Each of the 12,690 final rows still owns a record.

Under Valgrind’s two-level cache simulation, Imre incurred 1.65 million first-level data read misses per 10× pass and Rust incurred 0.55 million. This simulation used 32 KiB eight-way instruction and data caches, a 32 MiB sixteen-way last-level cache, and 64-byte cache lines.

Codon uses native concrete list and dictionary types, set materialization for dictionary-key union, native JSON and RE2, and Codon’s native `Path` for file resolution. Its benchmark uses signed 64-bit integers. The evidence includes the 20 generated Codon modules, their source transformations, and a separate native-platform build receipt for each link mode. [Codon adapters](../codon_native/README.md)

## Reproduction

Ruff is pinned at `de9071547cfec12f689142a7b8ce98edc7aef909` and the Python typing suite at `835b70f3f795f8e061b1f7621178515b0a77a0ad`. The changed case uses captured ty 0.0.60 and 0.0.90 diagnostics. The unchanged case uses identical captures. At 10×, each copy gets a unique basename and diagnostic aliases, including a distinct copy of the stub file. [Input preparation](../conformance_grouping_inputs.py) · [Differentials](../conformance_grouping_validate.py)

Measured source `7178431490cd6d8838b67daca89ccbdf03e7ce34`, previous native source `17845276508a0431276b2606f4f076dcd58ecf37`. Both link modes ran serially on CPU 31 of the same AMD EPYC-Milan Linux host. Each of 16 panels per mode ran six engines in 12 balanced orders, after one warmup per engine. Paired panels alternated the order of zero and repeated runs within each observation. Across both modes that is 3,456 measured processes and 288 warmups. Tools were CPython 3.14.5, native Codon 0.20.4, Ohm Rust 1.99.0-7, Clang 18.1.3, and LLD 23.1.1 for ThinLTO. Codon’s compiler flags are identical in both modes and its linked native platform library is rebuilt for each Rust link mode. [Benchmark](../conformance_grouping_benchmark.py)

The grouping probe exchanges the first and last old diagnostic every round and consumes every group’s index and three bucket lengths. The size checksum stays constant per pass on this capture. Callgrind recorded zero calls from the kernel driver to the grouping function in each preload control and four calls in each four-round previous Imre, current Imre, and Rust run. All six engines also produce complete transcripts containing each case key, all source labels, diagnostic fields, positions, expected tags, equivalence and per-file counts. [Timed driver](../conformance_grouping.py) · [Audit](../conformance_grouping_audit.py)

Extract the portable study into an empty directory and run `python3.14 -B tools/replay.py .`. It rechecks the original Ruff source, captures, build receipts, each process observation, all medians and gates, raw profiles, and the real and seeded upstream results. The replay relocates fixture paths while preserving the exact measured path spelling for output comparisons. [Portable study](2026-10-23-diagnostic-grouping-study.tar.xz) · [Ordinary observations](2026-10-23-diagnostic-grouping-ordinary.json.gz) · [ThinLTO observations](2026-10-23-diagnostic-grouping-thinlto.json.gz) · [Replay receipt](2026-10-23-diagnostic-grouping-audit.json)
