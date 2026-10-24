> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Native developer tools: download catalog and conformance reports

Two complete offline developer tools compile to native executables: uv's CPython download catalog and Ruff's typing-conformance report. Both meet the **≤2× Rust target** for complete runs and repeated full computation in ordinary and matched ThinLTO builds. Complete runs take **1.485–1.841× Rust's time ordinarily** and **1.326–1.767× with ThinLTO**; repeated computation takes **1.882–1.958×** and **1.655–1.909×**, respectively. These are measured medians across the four workloads below.

## Complete tools

Median milliseconds, including process startup, raw input reads, all parsing and computation, complete output writing, and exit. Each row uses 20 samples per engine after two warmups, measured at `88169978bfc9436024a6bfc34fc8a6c7de285023`. The target is assessed separately for ordinary and matched ThinLTO builds using unrounded ratios.

| Build | Workload | Upstream CPython ms | Typed CPython ms | Codon ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | CPython download catalog | 999.93 | 2972.91 | 567.21 | 286.26 | 155.45 | 1.841× |
| Ordinary | Unchanged conformance, GitHub | 153.21 | 251.58 | 58.66 | 35.29 | 23.77 | 1.485× |
| Ordinary | Changed conformance, GitHub | 155.72 | 252.68 | 59.84 | 36.82 | 24.31 | 1.515× |
| Ordinary | Changed conformance, diff | 155.30 | 251.39 | 59.18 | 36.11 | 24.05 | 1.501× |
| ThinLTO | CPython download catalog | 997.99 | 2979.12 | 568.82 | 214.84 | 121.56 | 1.767× |
| ThinLTO | Unchanged conformance, GitHub | 152.67 | 250.60 | 58.11 | 29.85 | 22.09 | 1.352× |
| ThinLTO | Changed conformance, GitHub | 155.40 | 253.00 | 60.32 | 30.99 | 22.72 | 1.364× |
| ThinLTO | Changed conformance, diff | 157.14 | 254.27 | 60.00 | 30.50 | 23.00 | 1.326× |

The catalog consumes the complete 19,870,417-byte python-build-standalone index: 567 releases and 58,721 raw artifacts produce 6,098 entries. Every run parses the raw rows and artifacts, filters and selects builds, sorts the catalog, and renders every JSON field. Fetching and lossless gzip decompression happen during preparation; no filtering or selection is prepared away.

Conformance inputs contain 146 Python typing tests and unchanged raw ty diagnostics. The unchanged workload compares ty 0.0.89 with 0.0.90, both producing 1,201 diagnostics. The changed workload compares 0.0.60's 1,169 diagnostics with 0.0.90's 1,201. Every run scans annotations, decodes diagnostics, groups required/optional/tagged/multi annotations, classifies results, calculates all case/file metrics, sorts, and renders the complete report. Preparation selects test files and resolves path aliases; running ty is outside the report timings. Both renderers force the summary table.

## Repeated complete computation

Separate entrypoints preload raw text once, then repeat the entire computation four times. Conformance repeats annotation scanning as well as diagnostic decoding; catalog repeats raw release/artifact parsing. Each pass contributes its complete output length to a checked checksum. The complete last output is written once and compared byte for byte; Rust black-boxes inputs and checksum contributions.

The table divides whole-process medians by four. Startup, preload, checksum, final output, and exit remain included. Matching zero-round controls write empty output and are retained separately; **nothing is subtracted**.

| Build | Workload | Upstream CPython ms/pass | Typed CPython ms/pass | Codon ms/pass | Imre ms/pass | Rust ms/pass | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | CPython download catalog | 865.86 | 2894.26 | 570.74 | 242.02 | 123.62 | 1.958× |
| Ordinary | Unchanged conformance, GitHub | 75.17 | 181.42 | 35.08 | 25.08 | 12.92 | 1.941× |
| Ordinary | Changed conformance, GitHub | 76.58 | 181.56 | 37.61 | 26.24 | 13.89 | 1.889× |
| Ordinary | Changed conformance, diff | 76.16 | 180.99 | 36.78 | 25.34 | 13.46 | 1.882× |
| ThinLTO | CPython download catalog | 871.47 | 2894.13 | 571.00 | 172.87 | 90.57 | 1.909× |
| ThinLTO | Unchanged conformance, GitHub | 75.27 | 181.18 | 35.13 | 19.39 | 11.38 | 1.704× |
| ThinLTO | Changed conformance, GitHub | 76.80 | 181.90 | 37.87 | 20.34 | 12.29 | 1.655× |
| ThinLTO | Changed conformance, diff | 75.91 | 181.02 | 37.53 | 19.84 | 11.99 | 1.655× |


## Provenance and comparison boundaries

The catalog's upstream CPython adapter supplies the retained HTTP response to uv's unchanged `CPythonFinder` and renderer from `83d556d712c7e316932b239c49e86a5de95d124a`; its raw input is pinned to astral-sh/versions `db0d0dbfa44ff1306ea0e571962e0b2a6211e3c9`. The typed port uses concrete records, typed JSON, immutable tuple keys, and stable sorting. The shared typed algorithm and Rust reference select winners while parsing and reuse each release's group lookup, preserving first-on-equal-priority selection and insertion order. Architecture names without a variant return directly instead of splitting, slicing, and joining the unchanged name. Variant parsing appends to one options list and skips splitting names without `+`; flag checks are reused, with suffix checks kept in their original order. PyPy, GraalPy, and Pyodide finders are outside this tool. See the [catalog guide](../../examples/python_catalog/README.md) for its stricter typed JSON boundary and rejected ambiguous empty architecture variant.

The conformance algorithm is from Ruff `de9071547cfec12f689142a7b8ce98edc7aef909`, using python/typing `835b70f3f795f8e061b1f7621178515b0a77a0ad`. Its upstream CPython adapter consumes captured inputs and uses the unchanged regex, models, classification, and renderers. The typed port uses concrete records and a Unicode-aware scanner in place of the regex. The offline CLI and quiet file output are disclosed adaptations, not the unchanged upstream CLI. See the [conformance guide](../../examples/conformance_report/README.md).

Upstream CPython and the typed CPython port are measured separately. Independent Rust implementations perform the same complete transformations and formatting. Native Codon 0.20.4 uses the shared typed application algorithms with recorded declaration/import adaptations, native JSON conversions, Unicode integer parsing, eager stable keyed sorting, and bounded URL percent decoding. Generated function-body ASTs are checked against the declared adaptations. No CPython bridge is enabled; validation points `CODON_PYTHON` at a nonexistent library. Codon's signed 64-bit integers and last-wins duplicate JSON keys limit its comparison domain; excluded adversarial cases remain checked in the other engines. The complete real outputs must match for every engine. See the [Codon contract](../codon_native/README.md).

## Method and validation

Measurements ran on 2026-10-21, on an x86-64 Linux AMD EPYC-Milan host pinned to CPU 31. The tools were CPython 3.14.5 with PGO/LTO and its JIT disabled, Clang 18, Rust 1.98.1-dev / Ohm 1.98.1-1 with experimental Cargo defaults disabled, and Codon 0.20.4. The compiler/runtime came from the strict clean build at `10b7b5a`; all 79 compiler/runtime source inputs were checked for exact equality with the measured application revision `8816997`. Imre and Rust use generic targets with matched ordinary/ThinLTO configurations. The same generic-CPU Codon release configuration is measured in both modes; it does not receive Imre/Rust's ThinLTO toggle.

Measurements run serially on one pinned CPU after builds, validators, and profilers stop. Five engines use ten balanced Williams orders, balancing positions and directed within-round neighbors; the 20 samples cover two complete cycles. Warmups and transitions between rounds are not balanced. CPython uses `-P -s -B`, `PYTHONHASHSEED=0`, and a disabled JIT in a minimal explicit environment. Each process's return code, stdout/stderr, complete output, and retained filesystem effects are checked outside the timer. Source, tool, binary, and complete input-tree guards are rechecked. Inputs are page-cache warm, outputs are not fsynced, and shared-host medians do not establish a confidence interval.

Each build-mode preparation passed **752 fixture engine executions**: 325 catalog and 427 conformance executions, plus 60 gates using the real workloads. Catalog coverage comprises 53 application and 14 compute cases; the seeded conformance corpus contains 30,000 annotation and 400 report differentials, reused across modes. The timed matrix contains **2,400 measured engine executions and 240 warmups**, including the zero-round controls; both modes also passed 120 timing gates in total.

Core validation passed 288 Rust unit tests, 36 ordinary language suites, 13 ASan suites with 943 checks, eight ThinLTO suites with 485 checks, and three instrumented sequence-ownership tests. Fully instrumented generated code and runtime passed 87 conformance and 67 catalog executions with leak detection. Source-equivalence records bind the unchanged core and conformance code to the final application revision; the final catalog sanitizer run used `8816997`.

The measured implementation pools immutable string literals, avoids intermediate JSON strings, copies contiguous string slices directly, fuses supported split/index operations, and stores three sequence words inline without growing the 48-byte runtime object on the measured 64-bit ABI. Callgrind CPU and DHAT allocation profiles guide these changes separately; their instrumented elapsed times are diagnostic only and are not benchmark samples.

The final ordinary catalog profile used the exact executables retained by strict preparation. Callgrind and DHAT recorded **1,775,384,590 instructions and 1,727,055 allocations** for Imre, versus **876,092,745 instructions and 885,672 allocations** for Rust. Relative to the prior architecture-shortcut version, the final application changes reduced Imre's instructions by **5.96%** and allocations by **10.12%**, with unchanged peak heap usage. The compiler/runtime were identical for that comparison. These are whole-program totals; inclusive helper percentages overlap. The independently checked profile records bind both executables to the strict ordinary preparation and retain the raw Callgrind/DHAT output. Earlier diagnostic comparisons remain in the evidence archive.

The earlier complete run at `10b7b5aae54b57b16eeb220f907e677af544df6c` measured repeated catalog work at **2.00024× Rust with ThinLTO**, narrowly above the numerical target. Its original records, source bindings, audit, and replay remain preserved as baseline evidence; rounding that ratio to 2.00× does not establish a below-2× result.

## Evidence and reproduction

The [ordinary raw samples](2026-10-21-native-tools-normal.json), [ThinLTO raw samples](2026-10-21-native-tools-thinlto.json), [timing context](2026-10-21-native-tools-context.json), and [independent audit](2026-10-21-native-tools-audit.json) retain the complete comparison. The [verification manifest](2026-10-21-native-tools-evidence.json) binds every published evidence file by size and SHA-256.

The single [portable archive](2026-10-21-native-tools-evidence.tar.xz) is **91,614,152 bytes (87.37 MiB)**, with SHA-256 `c51b6329869bac01c9432075e92182ca95158e73ca06e0d145ce43f610bd4273`. It retains 18,949 logical files as 2,994 unique blobs, including raw inputs, source snapshots, licenses, validation/profiling records, and earlier attempts. The [portable replay](2026-10-21-native-tools-replay.json) passed again after local transfer and produced byte-identical results. An [isolated replay](2026-10-21-native-tools-isolation.json) accessed no original paths and launched no native engines or network operations; [all 11 corruption probes](2026-10-21-native-tools-negative-probes.json) were rejected. The [preserved baseline replay](2026-10-21-native-tools-baseline-replay.json) also reproduces its original receipt exactly.

The [benchmark guide](../README.md#compute-heavy-developer-tools) describes preparation, frozen timing, auditing, and packaging. The evidence package preserves records and raw inputs byte for byte, includes the upstream licenses, and maps original path identities to content-addressed blobs. Portable replay reconstructs complete upstream outputs, checks every retained sample and filesystem effect, recomputes statistics/ratios/order balance, and checks ordinary/ThinLTO input parity. It needs CPython 3.14+ with assertions enabled, without Git, network, the original checkout, or native engines. Native executables and toolchains remain identified but excluded from portable rehashing; replay verifies the retained evidence rather than remeasuring performance.
From the repository root, extract the archive and replay its retained evidence:

```sh
tar -xf benchmarks/results/2026-10-21-native-tools-evidence.tar.xz
python3.14 native-tools-evidence-final-3/native_tools_evidence.py replay \
  native-tools-evidence-final-3 --output /tmp/imre-native-tools-replay.json
```
