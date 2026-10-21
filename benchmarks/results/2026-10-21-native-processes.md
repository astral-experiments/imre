# Native processes and prepared-project memory reports

The Ruff/ty memory-report port can now launch old and new ty binaries over prepared projects, capture their output, and produce the comparison itself. Complete runs take **1.00–1.61× Rust’s time**; separate repeated parse/compare/render workloads take **1.43–1.60× Rust’s time** across ordinary and ThinLTO builds.

The supported Linux subset is synchronous `subprocess.run` with an explicit argument list, captured UTF-8 streams, optional working directory and replacement environment, `check`, typed `CompletedProcess[str]`, and process exceptions. `os.environ.copy()` supplies a native environment snapshot. The result uses fixed-layout typed storage. Shell execution, timeouts, `Popen`, asynchronous processes, and arbitrary streams remain outside this milestone. See the [language contract](../../docs/subset.md) and [example guide](../../examples/memory_report/README.md).

## Complete process runs

Median milliseconds over 48 measured samples per engine, after two warmups. The real workload launches pinned ty 0.0.89 and 0.0.90 over retained inventory and Ruff Python sources. Synthetic workloads launch the same native producer for every engine and capture its complete JSON stdout and substantial stderr. Both synthetic layouts contain 16,256 report items per side: 64 small projects require 128 child launches; two larger projects require four.

| Build | Workload | CPython adapter ms | Relar ms | Rust ms | Relar/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | Real ty: inventory + Ruff scripts | 1056.766 | 982.700 | 965.604 | 1.018× |
| Ordinary | 64 projects × 254 items | 410.577 | 336.369 | 266.027 | 1.264× |
| Ordinary | 2 projects × 8,128 items | 190.399 | 85.401 | 58.030 | 1.472× |
| ThinLTO | Real ty: inventory + Ruff scripts | 1046.068 | 973.386 | 971.995 | 1.001× |
| ThinLTO | 64 projects × 254 items | 406.159 | 333.451 | 256.227 | 1.301× |
| ThinLTO | 2 projects × 8,128 items | 190.769 | 85.065 | 52.727 | 1.613× |

These complete runs are **1.07–2.24× faster than CPython**. The baseline is a prepared-project adapter using the unchanged upstream dataclasses and renderer, not the upstream repository-setup/download CLI. The typed CPython port is independently validated but excluded from the timed comparison.

Each sample includes startup, manifest validation, old/new launches in manifest order, full stdout/stderr capture and strict UTF-8 decoding, report parsing, comparison and stable ranking, Markdown rendering, one output write/close, and exit. The real workload includes all child ty checking time, so its ratio alone does not establish report-computation performance. The empty two-child control is retained in the raw results without subtraction.

## Repeated parse, compare, and render

Separate entrypoints preload report text and repeat the complete decode/compare/render work eight times without children. The table divides whole-process time by eight; preload, startup, checksum, and final output remain included. Every pass contributes its full report’s Unicode character count to a checked checksum. Rust black-boxes inputs and checksum contributions.

| Build | Workload | CPython adapter ms/pass | Relar ms/pass | Rust ms/pass | Relar/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | 64 projects × 254 items | 58.642 | 31.791 | 20.463 | 1.554× |
| Ordinary | 2 projects × 8,128 items | 59.023 | 29.648 | 20.799 | 1.425× |
| ThinLTO | 64 projects × 254 items | 58.235 | 28.179 | 17.645 | 1.597× |
| ThinLTO | 2 projects × 8,128 items | 59.034 | 27.081 | 18.485 | 1.465× |

The repeated workloads are **1.84–2.18× faster than CPython**. Matching zero-repetition preload controls are retained without subtraction. All six engine permutations occur eight times per workload, balancing positions and directed within-round neighbors. Warmups and transitions between rounds are not balanced.

## Semantics and validation

The application validates the entire project manifest before launching a child and finishes launch, decode, comparison, and rendering before opening its output. It copies the environment, enables ty JSON memory reporting, and fixes ty parallelism to one. Process failures, signals, missing executables/directories, invalid UTF-8, malformed reports, and output errors are checked against CPython. Existing file and directory modes remain supported. Upstream set-order ties are accepted only within the correct project section; complete report meaning and row selection are checked.

Both normal and ThinLTO preparation passed 542 comparison executions: 92 ordinary cases (335 executions), 32 process cases (127), 12 compute cases (48), and eight benchmark workloads checked with four engines (32). Full-runtime/generated-code AddressSanitizer validation passed 144 native invocations and 518 total comparison executions, with leak detection enabled. Expected-error paths were also scanned for sanitizer diagnostics.

All 261 compiler/runtime unit tests, formatting, Clippy, and 33 ordinary differential suites passed. Process tests passed 78 checks in ordinary O0/O3, ThinLTO O0/O3, and ASan O0/O1 modes. Seven affected suites passed 501 ThinLTO checks and 668 full-ASan checks. Ten focused normal/ASan probes covered argument/error precedence, empty and duplicate environment entries, result ownership in records/containers, and borrowed strings after argument mutation.

Measurements ran serially on CPU 31 after local builds and tests finished, outside the tool sandbox. CPython 3.14.5 uses PGO/LTO with its JIT disabled. Native code uses O3, Clang 18, and Ohm 1.98.1-1; Rust and Relar use matching ordinary/ThinLTO modes. Source, tools, child binaries, prepared inputs, and outputs are hash-guarded. Caches are warm, output is not fsynced, and validation/file hashing occurs outside the clock. This shared-host measurement does not establish a confidence interval.

The independent audit rechecked all 2,304 measured executions, 96 warmups, and 48 timing gates. Portable replay also passed with access to the original checkout/captures, native execution, and networking forbidden.

## Evidence and reproduction

Compiler, runtime, application, and measurement harness were frozen at clean commit `aca68f5631795f01c3cbb2dcdc56fd92866f6620`. The raw records retain every sample; hashes below identify uncompressed JSON.

| Record | SHA256 |
| --- | --- |
| [normal](2026-10-21-native-processes-normal.json.gz) | `b2a188534ef964eb01b42583c4760bc450023a4af6743530245b77038c499309` |
| [thinlto](2026-10-21-native-processes-thinlto.json.gz) | `c1d9785167635d15ce947d5d93a727f504a2a35dc5e38ef82d4f551ee47306f1` |

The [independent audit](2026-10-21-native-processes-audit.json), [manifest](2026-10-21-native-processes-evidence-manifest.json), [logical index](2026-10-21-native-processes-evidence-index.json), and [archive](2026-10-21-native-processes-evidence.tar.gz) retain source/Git identities, build and validation records, exact checked inputs/outputs, raw captures from both ty versions, and timing context. Historical benchmark results and native binaries/archives are omitted; their identities remain recorded.

Download the archive, index, manifest, and [replay script](2026-10-21-native-processes-replay.py) together:

```sh
python3 2026-10-21-native-processes-replay.py --prefix 2026-10-21-native-processes --output replay.json
```

Replay recomputes source identities, full output oracles, sample statistics, and order balance using retained bytes without Git, the original checkout, or native engines. It does not remeasure speed or prove historical host isolation. The [recorded replay](2026-10-21-native-processes-replay.json) and [isolation check](2026-10-21-native-processes-portable-check.json) retain its results. The [benchmark guide](../reference/memory_report/README.md) documents preparation and rerunning.
