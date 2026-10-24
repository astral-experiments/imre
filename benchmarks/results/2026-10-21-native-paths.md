> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Native paths and directory memory-report comparison

The real Ruff/ty memory-report port now compares directories using native `pathlib.Path`. The new complete-directory workloads take **0.91–1.28× Rust’s time**; repeated parse/compare/render workloads take **1.38–1.63× Rust’s time**. Both ordinary and ThinLTO builds meet the ≤2× target.

The compiler supports a sealed, immutable POSIX `Path`: construction and joining, `name`/`stem`/`suffix`/`parent`, predicates, explicit UTF-8 text I/O, `open(Path, ...)`, and immediately consumed `iterdir()` or single-component literal `glob()` pipelines. These are native runtime operations with typed ownership. Recursive globbing, stored path iterators, arbitrary path protocols, and non-UTF-8 filenames remain outside this subset. See the [language contract](../../docs/subset.md) and [example guide](../../examples/memory_report/README.md).

## Complete directory runs

Median milliseconds over 12 measured rounds per engine, after two warmups. Each side contains 65,024 items, laid out as 256 small projects or four larger projects. These synthetic project directories use deterministic copies of the genuine Ruff captures in the repository. Full cases compare old to new; unchanged cases still decode every report. Lower Imre/Rust is better.

| Build | Projects × items/project | Case | Upstream CPython ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | 256 × 254 items | full | 351.585 | 165.089 | 133.239 | 1.239× |
| Ordinary | 256 × 254 items | unchanged | 293.285 | 110.181 | 115.902 | 0.951× |
| Ordinary | 4 × 16,256 items | full | 335.262 | 140.205 | 121.439 | 1.155× |
| Ordinary | 4 × 16,256 items | unchanged | 261.086 | 91.445 | 100.109 | 0.913× |
| ThinLTO | 256 × 254 items | full | 348.398 | 148.001 | 115.855 | 1.277× |
| ThinLTO | 256 × 254 items | unchanged | 287.388 | 104.883 | 101.911 | 1.029× |
| ThinLTO | 4 × 16,256 items | full | 338.572 | 125.577 | 107.250 | 1.171× |
| ThinLTO | 4 × 16,256 items | unchanged | 255.987 | 88.713 | 86.236 | 1.029× |

These runs are **2.13–2.89× faster than the unchanged upstream CPython script**. The typed CPython port is also measured and validated in the raw records; its strict schema-conversion shim is separate from the upstream comparison above.

Every run includes process startup, directory scanning, reading and decoding all reports, common-project selection, comparison and stable ranking, full Markdown rendering, output write/close, and exit. The four application engines use balanced Williams orders. Caches were not flushed; warmups preceded measured samples. Output is not fsynced, and CPython runs use `PYTHONHASHSEED=0`. Full stream and filesystem validation runs outside the clock.

## Repeated parse, compare, and render

Separate entrypoints preload the input texts once and perform eight complete decode/compare/render passes. The table divides whole-process time by eight, including preload/startup/final output; it is an amortized estimate, not a pure in-process timer. Each pass contributes the full report’s Unicode character count to a checksum, and the checksum plus complete final report are checked. Rust black-boxes its borrowed inputs and checksum contributions. The original CPython adapter uses the pinned upstream models and renderer.

| Build | Projects × items/project | Upstream CPython ms/pass | Imre ms/pass | Rust ms/pass | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | 256 × 254 items | 238.101 | 132.467 | 81.433 | 1.627× |
| Ordinary | 4 × 16,256 items | 245.208 | 122.533 | 81.875 | 1.497× |
| ThinLTO | 256 × 254 items | 235.198 | 115.071 | 73.184 | 1.572× |
| ThinLTO | 4 × 16,256 items | 244.152 | 105.439 | 76.462 | 1.379× |

Zero-pass preload controls are retained without subtraction. Their medians are 3.1–6.3% of the corresponding native eight-pass medians. Repeated workloads use all six three-engine permutations over 12 rounds, balancing positions and directed within-round neighbors. All four engines, including typed CPython, are checked during preparation; repeated timing uses Imre, upstream CPython, and Rust.

The six existing file-mode workloads also passed in each build, taking 0.93–1.43× Rust’s time. Their complete timings and the empty-report controls are retained in the raw records. Small real captures are startup-heavy. Controls are retained without subtraction.

## Semantics and validation

The pinned upstream script is unchanged. The port preserves report thresholds, exact integer and percentage formatting, duplicate-item handling, and top-15 selection. It uses deterministic first appearance for tied rows; the validator allows upstream set-order ties only within the correct project section. Malformed unmatched reports are still errors, and parsing/rendering finish before an output file is opened. The Rust reference uses contiguous native integer records, checked arithmetic, and an arbitrary-precision fallback. Typed JSON’s documented strict boundary is unchanged.

Each normal/ThinLTO preparation passed 92 application cases (335 engine executions) and 12 compute cases (48 executions). The complete application passed the same cases under AddressSanitizer with leak detection. All 252 compiler/runtime unit tests, formatting, Clippy, and 32 ordinary differential suites passed. Path tests passed 67 checks at O0/O3, including 43 rejected programs, and also passed ThinLTO O0/O3 and ASan O0/O1. Seven affected file/record/container/iterator/callback/exception suites passed with ThinLTO and ASan. The local sandbox initially blocked LeakSanitizer’s tracing; the same checks passed outside it without disabling leak detection.

Measurements ran serially on CPU31 of an AMD EPYC-Milan devbox after local builds/tests finished, outside the tool sandbox. This is a shared-host comparison; spreads are not confidence intervals. CPython 3.14.5 has PGO/LTO enabled and JIT disabled. Native builds use O3, Clang 18, and Ohm 1.98.1-1. Rust references match ordinary/ThinLTO mode, use 16 codegen units, and retain the documented 1,000-instruction ThinLTO budget. Warmups and transitions between measured rounds are not balanced.

The independent audit rechecked 766 preparation executions, 112 timing gates, 224 warmups, and all 1,344 measured executions, including retained outputs, order balance, statistics, and source bindings. A separate portable replay passed with access to the original checkout and captures, subprocess execution, and networking forbidden.

## Evidence and reproduction

Compiler, runtime, application, and harness were measured from clean commit `cec52a5a963343f974ac774db5e9f25a7e7fc5ba`. The raw records retain every sample; SHA256 values below identify uncompressed JSON.

| Record | SHA256 |
| --- | --- |
| [normal](2026-10-21-native-paths-normal.json.gz) | `7a705acce5133f64dadd42b7a0e53cf7bb831872fdfe56820aeab2c41a25a8ad` |
| [thinlto](2026-10-21-native-paths-thinlto.json.gz) | `d77523a66d374b9b75666cfc0daf5d8af4d81edb86a82d2591265202c68dcdb1` |

The [independent audit](2026-10-21-native-paths-audit.json), [manifest](2026-10-21-native-paths-evidence-manifest.json), [logical index](2026-10-21-native-paths-evidence-index.json), and [archive](2026-10-21-native-paths-evidence.tar.gz) retain current source and Git identities, preparation/build records, checked inputs/outputs, validation logs, and timing context. Historical benchmark results and native executables/archives are omitted; native artifact hashes remain available.

Download the archive, index, manifest, and [replay script](2026-10-21-native-paths-replay.py) together, then run:

```sh
python3 2026-10-21-native-paths-replay.py --prefix 2026-10-21-native-paths --output replay.json
```

Replay verifies retained source/data and recomputes oracle results, statistics, and order balance without Git, native binaries, or original absolute paths. It does not rerun the engines or reconstruct host isolation. The [recorded replay](2026-10-21-native-paths-replay.json) and [isolation check](2026-10-21-native-paths-portable-check.json) retain the verification results. The [benchmark guide](../reference/memory_report/README.md) documents rebuilding and rerunning the experiment.
