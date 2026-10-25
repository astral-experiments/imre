> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Native allocation in Ruff’s diagnostic grouping

For Ruff’s isolated grouping pass over 1,460 files, Imre built with `--allocator jemalloc` used 1.066×–1.172× the time of Rust with the same allocator in the four ordinary/ThinLTO and short/long checkout combinations. That is 43.6–49.3% of the time taken by the same Imre application compiled from identical LLVM IR with the System heap. The largest ratio in the six-engine matrix was 1.545× for per-file statistics on 146 files (Ordinary) against system-allocator Rust. The largest against matched jemalloc Rust was 1.565× for per-file statistics on 146 files (Ordinary).

The measured Imre source is `aa24071e3bd7d2ad7b0051a7d873d0b405ac70e5`. The Python reporter, pinned Ruff upstream and both original comparator programs retain the preceding study’s 56 source identities. Linux native builds default to the System archive. `imre build script.py --allocator jemalloc` selects the separate self-contained archive used for the performance results below. The System archive contains the ordinary Rust system allocator. The separate jemalloc archive uses `tikv-jemallocator 0.6.1` with the same `background_threads_runtime_support` feature as the matched Rust reference. The application’s complete emitted grouping, report and compute LLVM modules are byte-identical to the preceding system-heap build in both link modes. ASan and macOS use the System archive. These timing measurements were taken on Linux x86-64. [Previous grouping study](2026-10-23-diagnostic-grouping.md) · [Pinned native allocator source](https://github.com/astral-sh/imre/blob/aa24071e3bd7d2ad7b0051a7d873d0b405ac70e5/crates/imre-runtime-jemalloc/src/lib.rs)

The release CLI was checked at `e483582`. Its relevant source trees match the integrated `e78c84a` checkout used for the live reporter. Rebuilding the 15 System, jemalloc and ASan program combinations with the recorded temporary LLVM filename reproduced their complete ELF bytes and emitted IR. The recorded filename preserves the ELF input-file metadata.

## Checkout path and allocation control

The same-host admission compares the preceding Ruff program on the System heap, the current program, original Rust and Rust using the identical allocator. The input has 12,690 diagnostic groups, 1,460 distinct source filenames and a separately checked, fully ordered diagnostic stream. The long and short checkouts have 71 and 24 shared path-prefix bytes respectively at this scale. Every median is the in-process work per pass from a fresh paired zero/12-round process observation. The eight engine/path treatments rotate through 16 balanced orders.

| Link | Checkout | Prior Imre ms | Imre ms | Rust System ms | Rust jemalloc ms | Imre / Rust System | Imre / Rust jemalloc |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Ordinary | long | 39.873 | 19.647 | 20.114 | 16.803 | 0.977× | 1.169× |
| Ordinary | short | 38.400 | 18.591 | 18.092 | 15.860 | 1.028× | 1.172× |
| ThinLTO | long | 35.955 | 15.663 | 17.382 | 14.689 | 0.901× | 1.066× |
| ThinLTO | short | 33.598 | 14.764 | 15.461 | 13.283 | 0.955× | 1.112× |

The default System heap reached 2.280× on the prior short-root ThinLTO treatment, above the 2× target. The current default reproduces every file-backed mapped ELF section of all six prior grouping, report and compute binaries except the 20-byte GNU build ID. Packed-record and borrowed-key experiments retained that limit and were excluded. The opt-in allocator’s 1,460-file complete report times in the eight same-schedule change-state, root and mode combinations were 0.838×–0.888× the time of the prior system-heap Imre. The archive contains that earlier candidate’s failed admission, the intermediate selector’s completed ordinary panel, four grouping comparisons and eight full-report comparisons across both checkout roots and link modes.

The prior typed-string port skipped filesystem resolution in `collect_file_stats`. With two path aliases for one source its validator produces five file rows where Ruff, current Imre and Rust produce four. The current grouping and statistics implementations and the Rust comparator resolve the filesystem paths on each measured pass. [Codon adapters](../codon_native/README.md) · [Ruff grouping comparison](2026-10-23-diagnostic-grouping.md)

## Six engines across both link modes

Times are milliseconds. CPython runs the original Ruff implementations and the current typed port. Imre before uses the genuine prior typed-string program at `1784527`. Native Codon 0.20.4 uses concrete lists and dictionaries, native JSON, RE2, native `Path` resolution and signed 64-bit integers. The original Rust implementation uses its normal System heap. Changed runs compare ty 0.0.60 with 0.0.90, and unchanged runs compare 0.0.90 with itself. All engines process the same source and analyzer records. Every complete report and ordered diagnostic dump is checked against Ruff. Each panel includes a warmup and 12 balanced measured observations per implementation. The two six-engine matrices retain 3,744 individual process runs, the matched three-engine matrices 1,872, and the two-root admission another 1,088. Kernel/repeated-compute timings subtract interleaved input/preload controls. Complete reports start a fresh CLI, read the saved analyzer outputs and source files, and publish the report. The live-checkout panel below measures running both ty analyzers as well. The matched-Rust ratio in the last column comes from an independent three-engine panel, with a fresh Imre numerator and both Rust heap choices.

### Ordinary

| Scale | Workload | Upstream CPython | Typed CPython | Imre before | Imre | Codon | Rust System | Imre / Rust System | Imre / Rust jemalloc |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1× | Group diagnostic records | 3.938 | 3.659 | 5.960 | 1.631 | 6.819 | 1.750 | 0.932× | 1.128× |
| 1× | Compare diagnostics | 1.458 | 1.464 | 0.127 | 0.106 | 1.767 | 0.092 | 1.146× | 1.305× |
| 1× | Collect file statistics | 9.487 | 9.236 | 0.299 | 4.467 | 6.229 | 2.892 | 1.545× | 1.565× |
| 1× | Complete report changed github | 162.823 | 265.599 | 36.056 | 34.436 | 67.075 | 26.180 | 1.315× | 1.361× |
| 1× | Complete report changed diff | 160.327 | 263.755 | 34.735 | 32.997 | 66.841 | 25.284 | 1.305× | 1.367× |
| 1× | Complete report unchanged github | 159.427 | 261.606 | 33.684 | 32.372 | 64.649 | 24.802 | 1.305× | 1.355× |
| 1× | Complete report unchanged diff | 161.152 | 264.071 | 34.245 | 32.438 | 64.107 | 24.845 | 1.306× | 1.368× |
| 1× | Repeat full report computation | 57.389 | 154.250 | 16.722 | 15.236 | 38.915 | 12.769 | 1.193× | 1.314× |
| 10× | Group diagnostic records | 75.515 | 55.987 | 84.046 | 19.852 | 88.504 | 19.945 | 0.995× | 1.173× |
| 10× | Compare diagnostics | 18.502 | 16.739 | 1.517 | 1.156 | 21.289 | 1.233 | 0.938× | 1.208× |
| 10× | Collect file statistics | 109.356 | 95.813 | 4.488 | 46.413 | 67.325 | 31.003 | 1.497× | 1.551× |
| 10× | Complete report changed github | 924.033 | 1,937.902 | 338.950 | 287.891 | 617.358 | 241.578 | 1.192× | 1.284× |
| 10× | Complete report changed diff | 914.025 | 1,915.183 | 326.836 | 284.472 | 608.832 | 235.456 | 1.208× | 1.280× |
| 10× | Complete report unchanged github | 901.696 | 1,928.771 | 318.025 | 277.271 | 599.131 | 232.820 | 1.191× | 1.265× |
| 10× | Complete report unchanged diff | 904.628 | 1,923.406 | 318.775 | 275.015 | 589.143 | 234.877 | 1.171× | 1.266× |
| 10× | Repeat full report computation | 704.803 | 1,699.210 | 206.788 | 165.927 | 480.624 | 155.800 | 1.065× | 1.084× |

### ThinLTO

| Scale | Workload | Upstream CPython | Typed CPython | Imre before | Imre | Codon | Rust System | Imre / Rust System | Imre / Rust jemalloc |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1× | Group diagnostic records | 3.874 | 3.511 | 4.662 | 1.202 | 6.851 | 1.503 | 0.799× | 0.996× |
| 1× | Compare diagnostics | 1.480 | 1.462 | 0.079 | 0.059 | 1.750 | 0.063 | 0.941× | 1.302× |
| 1× | Collect file statistics | 9.298 | 9.180 | 0.175 | 4.293 | 6.174 | 2.851 | 1.506× | 1.538× |
| 1× | Complete report changed github | 161.107 | 260.742 | 31.032 | 29.638 | 66.239 | 24.194 | 1.225× | 1.241× |
| 1× | Complete report changed diff | 161.321 | 261.300 | 30.643 | 29.806 | 66.647 | 23.996 | 1.242× | 1.273× |
| 1× | Complete report unchanged github | 158.476 | 263.187 | 30.102 | 29.095 | 64.617 | 23.676 | 1.229× | 1.296× |
| 1× | Complete report unchanged diff | 159.442 | 264.075 | 30.420 | 29.273 | 65.016 | 23.975 | 1.221× | 1.301× |
| 1× | Repeat full report computation | 57.938 | 154.090 | 13.392 | 12.119 | 38.802 | 11.574 | 1.047× | 1.160× |
| 10× | Group diagnostic records | 75.200 | 57.295 | 70.773 | 16.080 | 86.916 | 17.575 | 0.915× | 1.079× |
| 10× | Compare diagnostics | 17.808 | 16.307 | 0.970 | 0.679 | 20.995 | 0.763 | 0.890× | 1.250× |
| 10× | Collect file statistics | 103.818 | 103.277 | 2.704 | 43.329 | 65.981 | 30.534 | 1.419× | 1.495× |
| 10× | Complete report changed github | 943.638 | 1,935.736 | 296.678 | 250.114 | 616.248 | 229.205 | 1.091× | 1.192× |
| 10× | Complete report changed diff | 921.333 | 1,925.152 | 288.871 | 247.972 | 610.010 | 222.410 | 1.115× | 1.209× |
| 10× | Complete report unchanged github | 897.355 | 1,919.744 | 280.051 | 241.834 | 589.269 | 215.594 | 1.122× | 1.194× |
| 10× | Complete report unchanged diff | 893.632 | 1,915.750 | 280.760 | 241.274 | 591.333 | 215.800 | 1.118× | 1.200× |
| 10× | Repeat full report computation | 700.186 | 1,675.845 | 167.801 | 133.121 | 478.087 | 138.112 | 0.964× | 0.966× |

The primary and matched panels ran serially on CPU 31 of the same AMD EPYC-Milan Linux x86-64 host. Imre and Rust used clang 18.1.3 and Rust 1.99.0-dev (Ohm 1.99.0-7). The comparison engines included CPython 3.14.5 and AOT-native Codon 0.20.4. The matched three-engine panels retained the exact paired sample ordering, executable identities and both absolute native Rust denominators.

**Ordinary matched native milliseconds**

| Case | Imre | Rust System | Rust jemalloc | Imre / Rust jemalloc |
| --- | --- | --- | --- | --- |
| 1x/changed-github/group | 1.614 | 1.759 | 1.431 | 1.128× |
| 10x/changed-github/group | 19.849 | 20.269 | 16.916 | 1.173× |
| 10x/changed-github/compute | 164.262 | 152.933 | 151.504 | 1.084× |
| 10x/changed-github/report | 287.212 | 242.522 | 223.707 | 1.284× |

**ThinLTO matched native milliseconds**

| Case | Imre | Rust System | Rust jemalloc | Imre / Rust jemalloc |
| --- | --- | --- | --- | --- |
| 1x/changed-github/group | 1.222 | 1.511 | 1.228 | 0.996× |
| 10x/changed-github/group | 16.077 | 17.719 | 14.896 | 1.079× |
| 10x/changed-github/compute | 137.740 | 146.558 | 142.535 | 0.966× |
| 10x/changed-github/report | 251.850 | 226.224 | 211.273 | 1.192× |

## Memory, CPU and the live reporter

Peak process RSS was measured separately for the complete changed GitHub report. Every engine published the same file in each of six measured fresh processes plus a warmup per input and link mode. Values below are MiB (KiB / 1,024).

| Link | Scale | Older typed port | Prior Imre System | Imre jemalloc | Rust System | Upstream CPython | Typed CPython | Codon |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ordinary | 1× | 9.5 | 9.7 | 10.2 | 7.7 | 27.0 | 23.8 | 19.5 |
| Ordinary | 10× | 46.7 | 46.2 | 44.5 | 54.7 | 91.7 | 74.1 | 115.0 |
| ThinLTO | 1× | 10.2 | 10.5 | 11.5 | 8.2 | 27.0 | 23.8 | 19.5 |
| ThinLTO | 10× | 47.4 | 47.1 | 46.0 | 55.2 | 91.7 | 74.1 | 115.0 |

In the separate three-engine native RSS panel, all 84 published files matched the upstream oracle. Each cell is the median of six fresh processes after one warmup, using the same GNU time executable and isolated CPU.

| Native RSS, MiB | Scale | Imre jemalloc | Rust System | Rust jemalloc |
| --- | --- | --- | --- | --- |
| Ordinary | 1× | 10.1 | 7.7 | 9.0 |
| Ordinary | 10× | 44.6 | 54.7 | 60.5 |
| ThinLTO | 1× | 11.5 | 8.2 | 9.5 |
| ThinLTO | 10× | 46.0 | 55.2 | 58.8 |

On a separate Linux host, all four live programs and their subprocesses shared the same two cores. The genuine 146-file checkout ran both pinned ty versions, 0.0.60 and 0.0.90, for each complete process. Across 12 balanced measurements per engine, upstream CPython took 781.61 ms, default System Imre took 724.21 ms, Imre with jemalloc took 716.74 ms and Rust took 711.44 ms (1.018× and 1.007× respectively). Each of the 52 warmup/measured runs published the complete 109,258-byte changed GitHub output. All four genuine changed/unchanged GitHub/diff files also matched by byte across the four programs.

On the separate profiling host, ordinary Callgrind counted 170,194,324 guest instructions per 10× grouping pass on the old Imre System build and 143,359,872 on this Imre jemalloc build. The unchanged Rust System and matched Rust jemalloc took 169,523,366 and 145,405,522. The zero/four-round profiles directly count zero/four calls to the grouping function. The profiles count instructions and the RSS study measures process high-water. Statically linked jemalloc serves allocations internally, beyond libc-based DHAT interception. In the preceding System-heap DHAT runs the same pass allocated 126,670 blocks and 12.84 MiB in Imre, versus 172,695 blocks and 20.51 MiB in Rust.

## Effects on other programs

The allocator trade-off depends on the program. On a separate pinned core, unchanged Base45 decoding was about 9% slower with direct jemalloc and a minimal startup about 20% slower, with roughly 1 MiB additional peak RSS. The uv gzip catalog was about 3% faster with approximately 27% higher peak RSS. Selecting the separate System archive gives the original program sizes and startup RSS. Paired System decoding was within 0.1% of the preceding version. All outputs were checked against CPython. The raw source-bound control ships in the evidence archive.

| Control program | Pairs per arm | New System vs prior System | jemalloc vs prior System |
| --- | --- | --- | --- |
| Minimal process startup | 36 | +0.2% | +19.8% |
| Route planner, 96×96 grid | 12 | -1.7% | +2.1% |
| Base45 encoding, 16 KiB × 128 | 12 | -0.1% | +10.0% |
| Base45 decoding, 16 KiB × 128 | 36 | +0.0% | +8.8% |
| uv catalog, gzip, 1× | 12 | +0.3% | -3.1% |

## Reproduction

Extract the [offline study](2026-10-23-diagnostic-allocator-study.tar.xz) and run `python3.14 -B tools/replay.py .`. The replay compares the pinned upstream outputs, reconstructs all 1×/10× checks, recomputes the 32 six-engine medians and checks all 32 matched native cases. It verifies both checkout roots, CPU and RSS controls, source and native binary hashes, retained complete outputs and the two-archive build identities. The individual modality records and the fresh-replay audit are available separately. [Ordinary raw](2026-10-23-diagnostic-allocator-ordinary.json.gz) · [ThinLTO raw](2026-10-23-diagnostic-allocator-thinlto.json.gz) · [Audit](2026-10-23-diagnostic-allocator-audit.json)
