# Dataclass records in the real memory-report program

The memory-report port now uses dataclasses, a class method returning `Self`, a read-only property, and frozen difference records. On the large inputs it takes **1.16–1.47× Rust’s time**, meeting the ≤2× target with ordinary linking and ThinLTO. It runs **2.39–2.83× faster than the unchanged upstream CPython script**.

These are the same two-file report comparisons and typed JSON semantics as the [previous milestone](2026-10-21-memory-report.md). The upstream script, captured inputs, benchmark/oracle helpers, and optimized Rust references are unchanged. The [example guide](../../examples/memory_report/README.md) describes the supported program; the [Rust reference guide](../reference/memory_report/README.md) gives reproduction commands.

## Complete-program measurements

Median milliseconds over 12 measured rounds per engine, after two warmups. Lower Relar/Rust is better. The genuine inventory and Ruff captures contain 168 and 254 items per input file. The scaled cases use 32 or 256 distinct-name copies of the genuine Ruff capture: 8,128 or **65,024 items per input file**. They are synthetic larger inputs. “Unchanged” still reads and decodes both complete files.

| Build | Case | Original CPython | Typed CPython | Relar | Rust | Relar/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Ordinary | `real-inventory` | 77.092 | 61.103 | 3.342 | 3.168 | 1.055× |
| Ordinary | `real-ruff-scripts` | 75.711 | 64.423 | 4.639 | 4.395 | 1.056× |
| Ordinary | `scaled-full-32` | 114.186 | 356.589 | 23.490 | 18.869 | 1.245× |
| Ordinary | `scaled-unchanged-32` | 102.973 | 345.775 | 16.962 | 15.204 | 1.116× |
| Ordinary | `scaled-full-256` | 428.935 | 2486.435 | 179.393 | 123.334 | 1.455× |
| Ordinary | `scaled-unchanged-256` | 290.521 | 2426.102 | 108.788 | 93.426 | 1.164× |
| ThinLTO | `real-inventory` | 74.492 | 60.653 | 3.291 | 3.083 | 1.067× |
| ThinLTO | `real-ruff-scripts` | 76.077 | 64.310 | 4.248 | 3.319 | 1.280× |
| ThinLTO | `scaled-full-32` | 110.022 | 356.649 | 21.093 | 16.718 | 1.262× |
| ThinLTO | `scaled-unchanged-32` | 100.709 | 338.849 | 16.343 | 14.252 | 1.147× |
| ThinLTO | `scaled-full-256` | 429.301 | 2518.687 | 165.410 | 112.865 | 1.466× |
| ThinLTO | `scaled-unchanged-256` | 291.963 | 2412.897 | 103.189 | 86.865 | 1.188× |

Typed CPython includes the strict schema-conversion shim; the unchanged upstream script is the separate comparison above. Rust deserializes contiguous native integer records, uses checked arithmetic with an arbitrary-precision fallback, borrows names during comparison, and formats displayed rows. Relar retains arbitrary-precision integer semantics and its existing record allocation/ownership model.

Empty-report control medians, without subtraction:

| Build | Original CPython | Typed CPython | Relar | Rust |
| --- | ---: | ---: | ---: | ---: |
| Ordinary | 76.615 | 55.240 | 3.471 | 3.133 |
| ThinLTO | 75.751 | 55.883 | 2.961 | 2.937 |

The empty control reads and decodes two valid empty reports and writes a result. Its native median is 1.79–3.19% of the large-case median. It is comparable to the genuine small-case times, which are startup-heavy. The control excludes nonempty parsing/comparison and is not an exact fixed-cost component.

## Source changes and previous results

The models use compile-time `@dataclass` constructors with scalar defaults, keyword-only fields, and fresh list factories. `MemoryReport.from_json(...)` resolves its class receiver statically; the change total is a read-only property; displayed difference records are frozen. These preserve fixed layouts and direct native calls. Generated equality/repr, static methods, dictionary/set factories, and other supported options are covered by semantic tests rather than exercised by this timed workload.

Large Relar medians are within 1.4% of the previous measurements:

| Build | Case | Previous Relar ms | Current Relar ms | Change |
| --- | --- | ---: | ---: | ---: |
| Ordinary | `scaled-full-256` | 179.899 | 179.393 | -0.28% |
| Ordinary | `scaled-unchanged-256` | 107.331 | 108.788 | +1.36% |
| ThinLTO | `scaled-full-256` | 163.696 | 165.410 | +1.05% |
| ThinLTO | `scaled-unchanged-256` | 103.436 | 103.189 | -0.24% |

These are separate sessions, not an interleaved old/new experiment. The comparison is descriptive and does not isolate decorator or constructor overhead. No new CPU or allocation profiling is claimed.

## Semantics, method, and validation

The original threshold, size/percentage formatting, top-15 selection, arbitrary-size integer handling, JSON rejection/default rules, and report bytes remain covered. Upstream builds a set of names, so tied ranking/cutoff order can vary; the ports use stable first appearance. Validation permits only those upstream tie permutations, and `PYTHONHASHSEED=0` pins output during measurements. Upstream also echoes its file-mode report to stderr; that work is included and checked. Invalid input preserves an existing output report.

Each measurement includes process startup, imports where applicable, both file reads, decoding, comparison/ranking, rendering, output write/close, stream capture, and exit. There is no in-process repetition or startup subtraction. Filesystem/page caches are warm and output is not fsynced. Four Williams orders balance engine positions and directed within-round neighbors over 12 rounds; warmups and transitions between rounds are not balanced. Complete output/filesystem verification is outside the clock.

Runs were pinned to CPU31 on an AMD EPYC-Milan devbox, with local builds, tests, and profiles finished before timing. This is a shared-host workload/toolchain comparison; sample spreads are not confidence intervals. CPython 3.14.5 has PGO/LTO enabled and JIT disabled. Native builds use optimization level 3, Clang 18, and the pinned Ohm 1.98.1-1 Rust toolchain; ordinary and ThinLTO Rust references use the matching mode, 16 codegen units, and the documented 1,000-instruction ThinLTO budget.

Both preparations passed **72 application cases / 264 engine executions** each. The independent audit rechecked all 528 preparation results plus 56 gates, 112 warmups, and 672 measured captures, including retained streams/files, source/build/tool identities, command chronology, balanced order, raw statistics, and ratios. The complete application also passed 72 cases / 264 executions with the Relar executable built under AddressSanitizer and leak detection after timing.

The frozen implementation passed 247 Rust unit tests, formatting and Clippy, all 31 ordinary differential suites, and the six affected class/container/JSON/callback/formatting suites with ThinLTO and AddressSanitizer. The new dataclass suite covers 86 checks at `-O0`/`-O3`, including 46 rejected programs, ownership, exceptions, import aliases, lexical shadowing, and generated defaults. The shared-NaN dataclass oracle requires CPython 3.13+; older hosts skip only that fixture.

## Evidence and replay

Compiler/runtime, application, and harness were measured from clean commit `f4aee96d255b4b4772e8d3fd2ba7169844cc58eb`. The following raw records retain every sample; hashes refer to their uncompressed JSON.

| Record | SHA256 |
| --- | --- |
| [normal](2026-10-21-dataclass-records-normal.json.gz) | `244f754fc6a3162a527ad061c36d416a7eb6d2716fe36f8991f9559a8b8feb42` |
| [thinlto](2026-10-21-dataclass-records-thinlto.json.gz) | `9c60433ba2c3fcc41f19406058affa2508dd1aaa7c4a495d722e22a621c612c7` |

The [independent audit](2026-10-21-dataclass-records-audit.json), [evidence manifest](2026-10-21-dataclass-records-evidence-manifest.json), [logical file index](2026-10-21-dataclass-records-evidence-index.json), and [compressed archive](2026-10-21-dataclass-records-evidence.tar.gz) retain source snapshots/Git identities, preparation/build records, complete inputs and outputs, validation logs, and timing context. Native executables, runtime archives, bitcode, and bytecode are excluded; their recorded hashes remain available. Repeated contents are stored once.

Download the archive, index, manifest, and [portable replay](2026-10-21-dataclass-records-replay.py) together, then run:

```sh
python3 2026-10-21-dataclass-records-replay.py --prefix 2026-10-21-dataclass-records --output replay.json
```

Replay verifies the retained data and recomputes the oracle results, statistics, and order balance without native binaries, Git, or the original absolute paths. It does not rerun the measured engines or reconstruct historical host isolation. The original source/tool/binary guard checks are retained in the independent prepublication audit.
