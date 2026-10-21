# Compiling a real ty memory-report comparator

The typed port of Ruff/ty’s two-file memory-report comparator runs within **1.13–1.46× Rust’s time** on the large cases, meeting the ≤2× target in both ordinary and ThinLTO builds. These cases decode **65,024 items per input file**, compare the reports, and write the complete Markdown result. The unchanged upstream script also runs as a separate CPython reference.

This is a typed port of the useful comparison mode, with explicit report/item records and `relar.json.decode(text, MemoryReport)`. It is not a claim that the entire original script compiles unchanged. The [example guide](../../examples/memory_report/README.md) documents the supported CLI and schema; the [Rust reference guide](../reference/memory_report/README.md) describes the independent implementation and reproduction commands.

## Large inputs

Complete-program median milliseconds; 12 measured rounds per engine. Lower Relar/Rust is better.

| Build | Case | Original CPython | Typed CPython | Relar | Rust | Relar/Rust | Relar empty control |
|---|---|---:|---:|---:|---:|---:|---:|
| Ordinary | Full comparison | 428.651 | 2507.637 | 179.899 | 123.148 | 1.461× | 1.60% |
| Ordinary | Unchanged report | 295.035 | 2442.423 | 107.331 | 95.115 | 1.128× | 2.68% |
| ThinLTO | Full comparison | 439.783 | 2518.968 | 163.696 | 113.035 | 1.448× | 1.74% |
| ThinLTO | Unchanged report | 287.379 | 2409.657 | 103.436 | 85.775 | 1.206× | 2.75% |

The empty control reads and decodes two valid empty reports and writes a result. It is **not all fixed cost**: it excludes nonempty parsing and comparison. Its median is only 1.60–2.75% of Relar’s large-case median, so startup does not explain the large-case result. No control time is subtracted.

## All measured cases

The two genuine captures come from ty 0.0.89 and 0.0.90, running on identical source snapshots: this repository’s inventory example (168 items/file), and Ruff’s scripts directory (254 items/file). The scaled cases are explicitly synthetic: 32 or 256 copies of the genuine Ruff capture, with distinct item-name suffixes and scaled totals (8,128 or 65,024 items/file). “Unchanged” compares old with old; both files are still read and decoded in full. Exact producer commands, versions, source hashes, original JSON, and warning logs are retained in the [capture provenance](../../examples/memory_report/sample/provenance.json).

| Build | Case | Original CPython ms | Typed CPython ms | Relar ms | Rust ms | Relar/Rust |
|---|---|---:|---:|---:|---:|---:|
| Ordinary | `real-inventory` | 75.918 | 55.086 | 3.187 | 3.012 | 1.058× |
| Ordinary | `real-ruff-scripts` | 75.417 | 57.988 | 3.534 | 3.313 | 1.067× |
| Ordinary | `scaled-full-32` | 111.003 | 347.726 | 22.665 | 18.064 | 1.255× |
| Ordinary | `scaled-unchanged-32` | 103.395 | 337.131 | 16.776 | 15.262 | 1.099× |
| Ordinary | `scaled-full-256` | 428.651 | 2507.637 | 179.899 | 123.148 | 1.461× |
| Ordinary | `scaled-unchanged-256` | 295.035 | 2442.423 | 107.331 | 95.115 | 1.128× |
| ThinLTO | `real-inventory` | 77.436 | 56.655 | 3.214 | 3.159 | 1.017× |
| ThinLTO | `real-ruff-scripts` | 77.129 | 58.484 | 3.533 | 3.258 | 1.084× |
| ThinLTO | `scaled-full-32` | 115.363 | 345.272 | 21.154 | 17.065 | 1.240× |
| ThinLTO | `scaled-unchanged-32` | 104.910 | 347.572 | 17.339 | 14.551 | 1.192× |
| ThinLTO | `scaled-full-256` | 439.783 | 2518.968 | 163.696 | 113.035 | 1.448× |
| ThinLTO | `scaled-unchanged-256` | 287.379 | 2409.657 | 103.436 | 85.775 | 1.206× |

Empty-control medians (ms):

| Build | Original CPython | Typed CPython | Relar | Rust |
|---|---:|---:|---:|---:|
| Ordinary | 74.393 | 48.009 | 2.882 | 2.695 |
| ThinLTO | 75.142 | 49.088 | 2.848 | 2.752 |

The genuine small captures are startup-heavy: the native empty control is 80.62–90.43% of their median. Those rows demonstrate useful end-to-end latency, not sustained parsing throughput. Typed CPython includes the strict compatibility shim’s schema conversion, which is visible separately from the unchanged upstream script’s dynamic JSON processing.

## What changed

The first runnable port already met the target. Profiling then identified two removable costs: the JSON nesting-depth preflight scanned every string byte individually, and changed-item comparison hashed names once for membership and again for indexing. The runtime now searches quoted strings for quote/backslash bytes with vectorized `memchr`; the application uses one `.get` per side with explicit `None` guards. Arbitrary-precision integers, JSON rejection/default behavior, stable ranking, and report bytes are preserved.

The exploratory baseline used four measured rounds; the final result uses twelve. These are separate sessions, so the following before/after medians are descriptive rather than isolated causal estimates.

| Build | Large case | Baseline Relar ms | Final Relar ms | Reduction |
|---|---|---:|---:|---:|
| Ordinary | `scaled-full-256` | 199.929 | 179.899 | 10.02% |
| Ordinary | `scaled-unchanged-256` | 125.191 | 107.331 | 14.27% |
| ThinLTO | `scaled-full-256` | 195.276 | 163.696 | 16.17% |
| ThinLTO | `scaled-unchanged-256` | 118.148 | 103.436 | 12.45% |

Exact-binary Callgrind and DHAT runs used the 32-copy inputs in separate windows with no wall-timing runs:

| Case | Relar instructions before → after | Reduction | Final Rust instructions | Relar / Rust allocation blocks |
|---|---:|---:|---:|---:|
| `scaled-full-32` | 150,096,383 → 126,014,790 | 16.04% | 85,523,395 | 53,713 / 16,503 |
| `scaled-unchanged-32` | 106,665,569 → 87,021,924 | 18.42% | 74,296,962 | 48,925 / 16,327 |

On the full 32-copy input, dictionary lookup calls fell from 40,640 to 24,384 (40%). Decoder entry/preflight work, including its outlined search calls, fell from 34,273,070 to 14,607,861 guest instructions. Allocation block counts did not change; the six-byte requested-allocation difference is exactly the longer executable path stored in `argv[0]`.

JSON string scanning and typed record-map decoding remain prominent individual functions (11.58M and 9.27M self instructions on that input). Decoder-attributed allocation traffic remains 48,820 blocks / 2,281,528 requested bytes. Rust’s ordinary path deserializes contiguous native `i64` records, checks arithmetic, and falls back to arbitrary precision only on overflow; it borrows names while comparing and formats only displayed rows. Relar still pays for more runtime objects and allocations.

Callgrind counts guest instructions, not hardware cycles or elapsed-time shares. DHAT records requested allocation traffic, not RSS. The two changes were measured together; full-versus-unchanged differences are not exact phase accounting.

## Semantics and method

Both ports preserve the original threshold, size/percentage formatting, changed-item ranking, top-15 selection, and Markdown content. The original constructs a set of names, so equal-change order and selection at a tied cutoff are nondeterministic. The ports choose stable first-appearance order. Validation allows only those upstream tie permutations; `PYTHONHASHSEED=0` then pins complete upstream output across measurements. The upstream script also echoes the report to stderr in file mode, and that work is included and checked; the ports write the requested file quietly.

Rust uses the same typed JSON acceptance/default rules, including arbitrary-size integer tokens, known-field duplicate rejection, and depth 128. Normal inputs use checked native `i64` arithmetic; overflow retries the general implementation from retained input text. It preserves Python integer/float formatting semantics rather than truncating values to win the benchmark. No engine repeats or eliminates the application in process.

All runs are complete processes: startup, imports where applicable, two file reads, JSON decoding, comparison/ranking, rendering, output write/close, and exit. Four Williams orders balance engine positions and directed within-round neighbors across twelve rounds; warmups and cross-round transitions are not balanced. Full output and filesystem checks run outside the timer. Repeated measurements use warm filesystem/page caches. Output is written and closed without `fsync`, so these are not durable-storage latency measurements. CPU31 was pinned on an AMD EPYC-Milan devbox, with builds and profiles stopped. CPython 3.14.5 has PGO/LTO enabled and JIT disabled. Rust is pinned to Ohm 1.98.1-1; ordinary and ThinLTO builds use matching optimization modes, 16 codegen units, and the documented ThinLTO budgets. Spread in the raw records is not a confidence interval, and shared-host results are workload/toolchain-specific.

The final timing audit checked 56 gates, 112 warmup executions, and 672 measured executions (840 total, including controls). Each ordinary/ThinLTO preparation separately passed 72 application cases / 264 engine executions against the independent oracle, typed CPython, unchanged upstream on the shared domain, and Rust. Optimized validation additionally passed 241 workspace unit tests, fmt/clippy, typed-JSON ordinary/ThinLTO/ASan-O1/ASan-O0 checks (31/27/27/27), and the complete 72-case application suite with ASan-O1 and leak detection. Invalid-input tests are correctness evidence, not timed workloads.

## Evidence and replay

Measurements used clean application/harness revision `4fb67dcd36ae47c91be7e78fdc80cfd8b2f58b3a` and compiler/runtime source revision `7119fb22d97f7057e662d1735700256ff0c853be`. The exploratory application sources were subsequently committed as `73ec04b`; its compiler/runtime was `c28b93960b40ffeb51402ce02ec6cbf0d80dcd3b`. Exact historical source snapshots and source mappings are retained even though later test-only timeout fixes restacked the PR tips. The Rust binaries, Python/Clang identities, captures and benchmark/oracle helpers are unchanged between these sessions.

| Raw timing record | Uncompressed JSON SHA256 |
|---|---|
| [baseline normal](2026-10-21-memory-report-baseline-normal.json.gz) | `d1fffd17767d83638254c2c88cd142a5d37d3cab18a2c2c74211e78dd30c64ad` |
| [baseline thinlto](2026-10-21-memory-report-baseline-thinlto.json.gz) | `b0a18d19c2e49466c155023fb69ab4bd6622140574ec3b879cd1bf95c4d7638d` |
| [final normal](2026-10-21-memory-report-normal.json.gz) | `dbdbb9b6bc2e5ff9ce7a714c3d78b290d7449d0ca7595d7e2514924915fe7686` |
| [final thinlto](2026-10-21-memory-report-thinlto.json.gz) | `bfa7d1f8af6e87173e424853610f87ad9cc57991ea2baaa8bf743d576f21823a` |

The [timing audit](2026-10-21-memory-report-audit.json), [profile comparison](2026-10-21-memory-report-profile-comparison.json), [evidence manifest](2026-10-21-memory-report-evidence-manifest.json), and [logical file index](2026-10-21-memory-report-evidence-index.json) accompany the [compressed evidence archive](2026-10-21-memory-report-evidence.tar.gz). The archive retains preparation/validation/build records, source/tool identities, exact source snapshots, captured inputs and complete output bytes, raw profiles/logs, and audit/render helpers. Repeated file contents are stored once and indexed by logical path. Native executables and runtime archives are excluded and listed with their hashes; no binary downloads are needed to replay the data audit.

Download the archive, index and [portable audit](2026-10-21-memory-report-audit.py) together, then run:

```sh
python3 2026-10-21-memory-report-audit.py --prefix 2026-10-21-memory-report --output replay.json
```

The replay checks all four exact raw-record hashes, reconstructs the frozen Python oracle helpers, verifies complete retained outputs/files, and independently recomputes baseline/final statistics and order balance. It does not rerun native engines or reestablish the original host’s timing isolation; the prepublication audit retains those source/tool/binary guard results.
