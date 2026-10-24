> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Conformance reports directly from a typing checkout

The reporter now reads typed TOML configuration, discovers tests, runs two ty executables with deadlines, resolves diagnostic paths, computes the complete comparison, and publishes the report atomically. The measured checkout contains 146 tests. Complete workflows take **1.011–1.023× Rust’s time** across both build modes. Repeated full report computation takes **1.883–1.947× ordinarily** and **1.532–1.576× with ThinLTO**. All 18 measured cases, including zero-round controls, meet the **2× Rust target** using unrounded ratios.

The compiler/runtime changes are at `8bfc0b72fbfd26f92004d4a60f4ea48e289a0d7a`; the application and corrected native comparison helper are at `207ad26a179fdf2e3cd6e6ad2c555d49a603d329`. Imre's program build records name `4eed53a642d7230445bcfaed66c78ce40b93e8c1`: the later commit changes only the Rust/Codon process helper, and every retained Imre source and binary identity was rechecked against the measured checkout.

## Complete checkout workflow

Median milliseconds for fresh processes, including startup/imports, TOML parsing, test discovery, both actual ty children, path resolution, annotation and diagnostic processing, full rendering, atomic output replacement, temporary-directory cleanup, and exit. Each row uses 20 samples per engine after two warmups. Analyzer execution dominates this workflow; the separate computation table below exposes the reporter's own cost.

| Build | Workload | Upstream CPython ms | Typed CPython ms | Codon ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | Unchanged, GitHub | 978.71 | 1283.81 | 949.18 | 885.96 | 872.89 | 1.015× |
| Ordinary | Changed, GitHub | 997.04 | 1302.16 | 969.38 | 907.78 | 888.46 | 1.022× |
| Ordinary | Changed, diff | 993.01 | 1288.76 | 961.67 | 899.36 | 885.45 | 1.016× |
| ThinLTO | Unchanged, GitHub | 980.90 | 1290.07 | 943.78 | 885.46 | 865.47 | 1.023× |
| ThinLTO | Changed, GitHub | 1008.44 | 1310.28 | 970.77 | 899.97 | 890.49 | 1.011× |
| ThinLTO | Changed, diff | 1008.31 | 1302.51 | 980.49 | 903.38 | 884.67 | 1.021× |

The unchanged workload runs ty 0.0.90 twice. Changed workloads compare ty 0.0.60 with 0.0.90: 1,169 versus 1,201 diagnostics. The test suite is python/typing `835b70f3f795f8e061b1f7621178515b0a77a0ad`; the upstream reporting algorithm is from Ruff `de9071547cfec12f689142a7b8ce98edc7aef909`. Both report formats force the summary table. Every engine must produce the same complete report bytes.

## Repeated complete report computation

Separate entrypoints preload unchanged captured source text and raw diagnostics, then repeat annotation scanning, JSON decoding, grouping, classification, sorting, metrics, and rendering four times. Each complete output contributes its character count to a checked checksum; the last complete report is written and compared byte for byte. Rust black-boxes inputs and checksum contributions. Analyzer children do not run in this measurement.

The table divides whole-process medians by four. Startup, preload, checksums, final output, and exit remain included. **No zero-round baseline is subtracted.**

| Build | Workload | Upstream CPython ms/pass | Typed CPython ms/pass | Codon ms/pass | Imre ms/pass | Rust ms/pass | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | Unchanged, GitHub | 74.00 | 181.36 | 33.91 | 24.35 | 12.51 | 1.947× |
| Ordinary | Changed, GitHub | 75.28 | 180.58 | 35.45 | 25.50 | 13.44 | 1.898× |
| Ordinary | Changed, diff | 75.94 | 180.20 | 34.69 | 24.84 | 13.19 | 1.883× |
| ThinLTO | Unchanged, GitHub | 74.05 | 180.17 | 33.59 | 18.27 | 11.59 | 1.576× |
| ThinLTO | Changed, GitHub | 75.13 | 179.39 | 34.55 | 19.25 | 12.39 | 1.554× |
| ThinLTO | Changed, diff | 75.56 | 179.96 | 34.55 | 18.81 | 12.28 | 1.532× |

Matching zero-round controls load frozen inputs and write an empty report:

| Build | Workload | Upstream CPython ms | Typed CPython ms | Codon ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | Unchanged, GitHub | 95.72 | 84.58 | 19.50 | 10.69 | 9.79 | 1.092× |
| Ordinary | Changed, GitHub | 96.64 | 85.23 | 19.52 | 10.82 | 10.03 | 1.079× |
| Ordinary | Changed, diff | 96.22 | 83.84 | 19.14 | 10.46 | 9.63 | 1.086× |
| ThinLTO | Unchanged, GitHub | 95.03 | 83.77 | 19.31 | 10.27 | 9.55 | 1.075× |
| ThinLTO | Changed, GitHub | 95.07 | 83.93 | 19.13 | 10.22 | 9.50 | 1.076× |
| ThinLTO | Changed, diff | 96.70 | 85.36 | 20.20 | 10.69 | 10.16 | 1.052× |

## New capability and comparison boundaries

`imre.toml.decode(text, Schema)` decodes TOML 1.1 into concrete record or dictionary schemas with exact scalar types, optional fields, and supported defaults. Ignored values still undergo syntax and depth validation. The native parser is the pinned maintained `toml` crate; the CPython shim uses `tomllib` on Python 3.15+ or `tomli>=2.4` on older Python. Arbitrary dynamic TOML trees and datetime-valued schema fields remain outside the supported API.

`Path.is_absolute()` and `Path.resolve(strict=False)` support lexical checks and filesystem resolution, including relative paths and symlinks. `subprocess.run(timeout=...)` accepts a float or `None`; typed `TimeoutExpired` handling exposes the original command, timeout, and partial byte captures. On timeout the runtime kills and reaps the direct child and closes its capture pipes. The reporter validates inputs, stages output in a sibling temporary workspace, and replaces the destination only after the complete report succeeds.

The typed application is shared between Imre and CPython. The upstream CPython adapter retains Ruff's reporting algorithm with a bounded checkout CLI, timeout handling, and atomic-output adaptation; it is not the unchanged upstream command line. Native Codon 0.20.4 compiles the typed algorithm with the documented declaration/import adapters and native JSON conversions. For this workflow, Codon calls a separately built Rust library for TOML, captured subprocesses/deadlines, and temporary directories. The Rust reference shares that OS/TOML helper but independently implements report computation. FFI and conversion costs remain timed, and no native engine links CPython. See the [application guide](../../examples/conformance_report/README.md) and [Codon comparison contract](../codon_native/README.md).

## Method and validation

Measurements ran on October 21, 2026 in America/New_York (October 22 UTC), on an x86-64 Linux AMD EPYC-Milan devbox pinned to CPU 31, matching the preceding developer-tools comparison. Builds and other validators finished before timing. Ordinary and ThinLTO modes ran serially. Tools were CPython 3.14.5 with PGO/LTO and its JIT disabled, Clang 18, Rust 1.98.1-dev / Ohm 1.98.1-1 with experimental Cargo defaults disabled, and Codon 0.20.4. Imre and Rust use generic targets and matched ordinary/ThinLTO settings. Codon keeps the same portable release flags in both modes; its linked OS/TOML library follows the corresponding Rust mode.

Five engines use ten Williams orders that balance positions and directed within-round neighbors; 20 samples cover two cycles. Warmups and transitions between rounds are not balanced. CPython uses a minimal explicit environment, `-s -B`, and `PYTHONHASHSEED=0`. Typed and offline entrypoints also use `-P`; the upstream live adapter retains its script directory for sibling imports. Every process's status, stdout/stderr, complete output, and filesystem effects are checked outside the timer. Source, tool, binary, shared-library, checkout, and captured-input identities are guarded before and after measurement. Filesystem caches are warm; publication is atomic but outputs are not fsynced. Shared-host medians are observations, not confidence intervals.

Both final modes passed 85 live validation executions apiece across five engines. Cases cover real changed/unchanged reports, relative symlinks, malformed TOML and diagnostics, invalid UTF-8, missing cases, unknown diagnostic paths, child failures, timeouts with verified child reaping, publication errors, cleanup, and preservation of existing output. The timing matrix contains 1,800 measured executions and 180 warmups, including controls.

Core checks passed 293 Rust unit tests and three native-platform tests. Fully instrumented generated code and runtime passed 397 regression checks with ASan/LSan; the instrumented live reporter passed 51 additional engine executions. Retained offline validation covers 397 checks plus 30,000 annotation and 400 report differentials.

An initial incomplete diagnostic timing attempt is retained separately and excluded from these tables. It exposed an avoidable zero-descriptor poll delay in the Rust/Codon helper. The helper now drains pipes before checking child status and uses a short wait after both streams close; a regression test covers that sequence. Both native references were rebuilt before the final measurements. Imre's programs were unchanged.

## Evidence and reproduction

The [ordinary samples](2026-10-21-live-conformance-normal.json.gz), [ThinLTO samples](2026-10-21-live-conformance-thinlto.json.gz), [strict audit](2026-10-21-live-conformance-audit.json), and [portable replay receipt](2026-10-21-live-conformance-portable-check.json) retain the complete comparison. The [verification manifest](2026-10-21-live-conformance-evidence.json) binds the published files by size and SHA-256.

The [portable archive](2026-10-21-live-conformance-evidence.tar.xz) is **5,124,912 bytes** (4.89 MiB), with SHA-256 `a752489aeff35e007d27f5ea19f7d2f762ccf7d2edb860312cf9c1998712491c`. It includes raw samples, complete report and capture trees, build records and logs, committed source snapshots, the pinned typing checkout, upstream licenses, validation evidence, and the superseded diagnostic attempt. It excludes native executables, runtime archives, generated LLVM IR, and installed toolchains; their recorded identities remain available.

The strict audits hashed 1,144 and 1,145 files with no missing or skipped inputs. Portable replay verified 5,125 indexed entries and 680 source files, then reproduced the strict audit's timing statistics and checksums. After the final archive was extracted with the command below, replay produced a byte-identical receipt. Portable receipts explicitly count unavailable or unrebased external binary/toolchain paths.

The independent audit recomputes sample ordering, counts, medians, ratios, output hashes, and nonzero computation checksums; it also checks retained validation and capture trees. It does not execute native programs, rebuild compilers, or reconstruct the upstream oracle algorithm. Strict auditing ran on the measurement host; portable auditing checks the retained files after rebasing paths and reports unavailable toolchain/binary identities explicitly.

From the repository root, extract and audit with Python 3.12 or newer and assertions enabled. `-p` preserves the recorded filesystem permissions.

```sh
tar -xpf benchmarks/results/2026-10-21-live-conformance-evidence.tar.xz
python3 -B live-conformance-evidence-20261021/replay.py \
  > /tmp/imre-live-conformance-replay.json
```

The [benchmark guide](../README.md#live-conformance-workflow) and [application guide](../../examples/conformance_report/README.md) contain build and run commands. Raw build records retain exact compiler, linker, runtime, source, environment, and tool hashes.
