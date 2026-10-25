# Benchmarks

The [Rust, CPython and Imre comparison](runtime-suite.md) covers six workloads with checked outputs, raw samples and a command to reproduce the tables and chart.

## Packed value structs

[Packed struct lists vs frozen dataclasses](results/2026-10-25-struct-points.md) measures
construction, repeated scans, and peak process memory on Imre and CPython. It includes
independently constructed points, a list created by repeating one point, and the first indexed write that changes a repeated value.
The [harness](struct_points.py) checks every sample against an independent checksum.

[When to pack sparse edits](results/2026-10-25-struct-edit-density.md) compares
clustered and scattered replacements at several densities. It times writes,
the single write that crosses the packing threshold, twelve full scans, and
65,536 indexed reads against an already packed list of identical points.

## Route planner

The example computes shortest routes across weighted grids using Dijkstra's algorithm and its own binary heap. CPython and Imre execute the same `examples/route_planner.py` source, receive the same input, and must produce exactly the same output. The compiler generates LLVM IR and links a native executable against the independent Rust runtime. There is no rewritten native kernel or CPython interpreter inside that executable.

The [first recorded result](results/2026-10-17-route-planner.md) includes raw samples and environment details: 2.17× faster than CPython 3.14.5 for three routes on 256 × 256 grids, using end-to-end process timings.

The [profiling follow-up](results/2026-10-17-route-planner-optimized.md) records the native bottlenecks and their optimization: 2.60× faster than the preserved original native executable and 5.66× faster than CPython in a direct three-way comparison, with the Python source unchanged.

Build the compiler and runtime once, then run the benchmark from the repository root:

```sh
cargo build --release --workspace --locked
python3.14 benchmarks/benchmark.py \
  --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a \
  --python python3.14 \
  --clang clang \
  --output benchmarks/local/route-planner.json
```

Use the repository's pinned Rust toolchain and LLVM 18 or a compatible Clang installation. On a development machine configured to require Ohm, use `cargo +ohm build --release --workspace` instead and pass the compiler/runtime paths from that checkout's `CARGO_TARGET_DIR`. The harness itself requires Python 3.11 or newer; `--python` independently selects the CPython executable being measured. The current harness requires Linux for CPU affinity and its tracing check.

The default workloads use grid widths 32, 128, and 256, with three routes per process and seed 42. `--sizes`, `--routes`, and `--seed` select other workloads. Each workload has two warmup pairs followed by eleven measured pairs; `--warmups` and `--samples` change those positive counts. `--cpu` selects an allowed CPU; otherwise the first allowed CPU is used. The harness and all children are pinned to that CPU. `--timeout` sets the per-process timeout for compilation, probes, warmups, and samples; it defaults to 120 seconds. The independent validator uses its own 120-second process timeout. Generated binaries, LLVM IR, and default JSON output live under the ignored `benchmarks/local/` directory; `--artifact-dir` changes the binary and IR location.

Before timing, the harness compiles once at LLVM `-O3` and runs the differential validator. Validation compares complete output against CPython and checks shortest-path costs against an independent Bellman-Ford implementation on small grids, including large positive and negative seeds. Each measured workload also gets a CPython reference output. Every warmup and measured execution must exit successfully, emit no stderr, and match that entire output. The harness aborts if it is running under a ptrace tracer; run it outside the tracing sandbox so tracing overhead is not mistaken for execution time.

The timer surrounds `subprocess.run` with `time.perf_counter_ns`. Each sample therefore includes fresh process startup, input/output pipes, all route-planner work, and process exit. CPython runs as `python -I examples/route_planner.py`, including interpreter startup and source compilation on every invocation; Imre runs its previously compiled executable. These are end-to-end command-line timings. They do not isolate the algorithm's steady-state execution time. AOT compilation, correctness checks, and warmups are excluded from execution timings. AOT compilation time is reported separately and includes Imre checking/lowering, LLVM optimization, linking, and output creation; it excludes building the Rust compiler and runtime.

Within each pair, execution order alternates between CPython first and Imre first. The JSON stores every raw duration and output, all warmup pairs, input text, source/input/executable/IR/tool hashes, Git revision and dirty status, tool versions, available CPython JIT/GIL/PGO/LTO configuration information, CPU affinity/model, and OS details. PGO/LTO fields describe recorded configure options and compiler flags rather than proving how an interpreter binary was built. Source and tool hashes are checked again after measurement to reject changes during the run. Hostnames and complete Python configure arguments are not collected.

Results report each implementation's median, minimum, maximum, and interquartile range, plus CPython's median divided by Imre's median. Observed spread is not a confidence interval. CPU affinity and alternating order reduce some noise, but a shared devbox still has scheduling interference, frequency changes, and shared cache/memory contention; stop concurrent builds and other experiments before measuring. This is evidence about this route planner, input sizes, and recorded toolchain. It does not establish a general Python speedup, extension compatibility, or a safety guarantee.

## Comparing native changes

Preserve the baseline executable before rebuilding. `compare.py` runs that executable, an optimized executable, and unchanged CPython in the same session:

```sh
python3 benchmarks/compare.py \
  --baseline benchmarks/local/profile-baseline/route_planner \
  --optimized benchmarks/local/artifacts/route_planner \
  --baseline-record benchmarks/results/2026-10-17-route-planner.json \
  --optimized-record benchmarks/local/route-planner.json \
  --python python3.14 \
  --output benchmarks/local/comparison.json
```

The optional build records must match the current source and executable hashes. The comparison does not rebuild either binary. It uses the same correctness oracle, timing boundaries, CPU affinity, and tracing guard as `benchmark.py`. Defaults are two warmup triplets and twelve measured triplets, covering all six execution orders twice. `--samples` accepts other positive counts; a multiple of six keeps every implementation equally represented in each execution position. The JSON records each order and raw duration, hashes, native build provenance when supplied, and both optimized-to-baseline and optimized-to-CPython ratios.

## Profiling the executable

Callgrind can collect instruction and call counts without hardware-counter access. Profile the same prebuilt executables and inputs, keeping instrumented profiling separate from normal timing:

```sh
printf '256\n3\n42\n' > benchmarks/local/profile-input.txt
valgrind --tool=callgrind --dump-instr=yes --collect-jumps=yes \
  --callgrind-out-file=benchmarks/local/callgrind.out \
  benchmarks/local/artifacts/route_planner < benchmarks/local/profile-input.txt
callgrind_annotate --auto=no --inclusive=no --threshold=100 benchmarks/local/callgrind.out
```

Callgrind reports executed instructions, not CPU cycles or elapsed-time shares. Rust inline source locations may split one function across several rows; aggregate them by function when comparing totals. LLVM may inline generated Python functions into their callers, and identical runtime functions can share a symbol address: `imre_list_retain` and `imre_str_retain` currently do so. Inspect symbols and callers before attributing that shared entry to strings.

## Dependency planner

The [dependency-planner package](../examples/dependency_planner/README.md) exercises static imports, typed classes, general lists, tuples, and optional values in a complete scheduling program. Its independent oracle and `dependency_benchmark.py` compare the unchanged package on CPython and Imre, using the same fresh-process timing, single-CPU affinity, and tracing guard. See the package README for input semantics, correctness checks, reproduction commands, and the compact per-sample output records.

The [recorded comparison and I/O profile](results/2026-10-17-dependency-planner.md) include six workloads and a preserved native baseline. At 16,384 tasks, buffered Imre runs the layered graph 2.39× faster than CPython and 10.97× faster than the original native build. The three-way harness rotates all six execution orders and verifies every complete report.

The [CPU and allocation follow-up](results/2026-10-17-dependency-performance.md) compares optimized Imre with CPython and algorithm-matched Rust references. At 16,384 tasks, optional ThinLTO makes Imre about 1.5× faster than the buffered baseline and 3.69×/5.26× faster than CPython on layered/wide graphs. It approximately matches arbitrary-precision Rust on the layered graph and remains 46% slower on the wide graph, with LLVM optimization budgets matched. The report includes ordinary-link controls, cumulative allocation profiles, correctness checks, and raw samples.

The [range-proof and container-access follow-up](results/2026-10-17-dependency-index-specialization.md) reduces large wide-graph time by another 20%, narrowing the matched Rust BigInt gap from 46% to 16%. Imre is 4.04×/6.67× faster than CPython on large layered/wide graphs with ThinLTO. The report preserves ordinary-link controls, 30-sample raw comparisons, cumulative CPU/allocation profiles, and validation of bounds, ownership, aliases, and arbitrary-precision fallbacks. The Python package and Rust reference algorithm remain unchanged.

The five-way `dependency_compare.py` comparison and native correctness suites accept optional `--lto-linker /path/to/ld.lld --target x86_64-unknown-linux-gnu` flags. Supply a compatible runtime archive; these harnesses do not rebuild it. The comparison records the requested and effective target, build flags, and linker version and hash. Linker symlink names are preserved because they select LLD's flavor. Omitting these flags retains the default linking behavior.

`dependency_profile.py` profiles preserved executables using the inputs and complete expected outputs from a comparison record. For example, after producing `benchmarks/local/dependency-comparison.json` and its native artifact:

```sh
python3 benchmarks/dependency_profile.py \
  --record benchmarks/local/dependency-comparison.json \
  --binary optimized=benchmarks/local/dependency-comparison/dependency_planner \
  --sizes 16384 --shapes layered wide \
  --output benchmarks/local/dependency-profile
```

This runs Callgrind and DHAT separately, requires a new output directory, and verifies exit status, complete output, and executable hashes. Supply `--build-record optimized=/path/to/build-record.json` to preserve additional build provenance. Instrumented runs are excluded from timing results; the output includes trace/log hashes and instruction/allocation summaries. Valgrind is discovered on `PATH` or in the Imre tool cache, or can be selected with `--valgrind` and `--valgrind-lib`.

## Incremental planner

The [named incremental planner](../examples/incremental_planner/README.md) uses dictionaries, sets, and direct iteration to validate a named graph and schedule the targets affected by changes. The unchanged package runs on CPython and Imre; [Rust references](reference/incremental_planner/README.md) implement the same algorithm with either arbitrary-precision or checked-i64 arithmetic.

The [recorded comparison](results/2026-10-17-incremental-planner.md) includes ordinary and matched ThinLTO controls, all raw samples, frozen build records, and sanitizer validation. Imre runs the four 16,384-target workloads 2.28–4.67× faster than CPython with ThinLTO and takes 1.49–1.57× as long as Rust BigInt. These are whole-process results for the new named planner, including whole-graph validation and complete output.

Build and validate all four implementations before measuring:

```sh
target/release/imre build examples/incremental_planner/__main__.py \
  --module-root examples --runtime target/release/libimre_runtime.a \
  --clang clang-18 --opt-level 3 -o /tmp/incremental-planner
python3 benchmarks/reference/incremental_planner/build.py \
  --artifact-dir benchmarks/local/incremental-reference-normal \
  --target-dir target/incremental-reference
python3 benchmarks/incremental_validate.py --binary /tmp/incremental-planner \
  --rust-bigint benchmarks/local/incremental-reference-normal/planner-bigint \
  --rust-i64 benchmarks/local/incremental-reference-normal/planner-i64 \
  --python python3.14
```

Validation checks 155 graph cases and 17 separate parser cases, including complete output and fatal error categories. Checked i64 matches 138 graph cases and must explicitly reject the other 17 for overflow; all parser cases are in range. The reference guide gives ordinary and ThinLTO build commands, local Ohm options, and `incremental_benchmark.py` reproduction commands. Its four-engine comparisons use 32 measured rounds with balanced Williams orders, two warmups, one pinned CPU, fresh processes, exact output checks, and preserved source/tool/build provenance. Default workloads cover sparse and full changes in 16,384-target layered and wide graphs plus a smaller integer-boundary case. These are whole-process measurements, including parsing, whole-graph validation, scheduling, and the complete report.

## Batch planner and exception recovery

The [framed batch planner](../examples/batch_planner/README.md) adds bounded request parsing, catchable errors, and `finally` cleanup while importing the existing graph algorithm unchanged. Its independent oracle checks 194 inputs against CPython, Imre, and Rust BigInt, including malformed requests followed by valid work and terminal framing failures.

`batch_benchmark.py` measures two groups: frozen old/new Imre and CPython on the unchanged single-request source, then Imre/CPython/Rust BigInt on valid and mixed batches. The first isolates compiler/runtime regression from the new program's framing and buffering work. Each group uses 30 rounds with all six engine orders, two warmups, one CPU, complete output comparisons, and recorded build provenance. The package guide contains build and reproduction commands for separate ordinary and matched ThinLTO configurations.

The [recorded results](results/2026-10-18-batch-planner.md) preserve both comparisons and the measured regression: large unchanged-source workloads take 6.4–9.8% more time with ordinary linking and 8.4–22.7% more with ThinLTO. Large valid/mixed batches run 2.31–3.94× faster than CPython with ThinLTO and take 1.70–1.82× Rust BigInt's time. Separate DHAT evidence compares the same binary at 800 and 80,000 recovered failures; peak live requested heap remains 18,140 bytes. Generated code and the Rust runtime also pass full AddressSanitizer/LeakSanitizer validation.

`exception_benchmark.py` reruns the exact captured inputs after optimizing propagation. Its unchanged-program group includes CPython, the frozen pre-exception executable, the frozen post-exception executable, and the optimized executable. Its batch group includes CPython, frozen/optimized Imre, and the same Rust BigInt executable. Each group uses 32 rounds with balanced four-engine Williams orders and two warmups. The harness verifies the complete baseline provenance chain, unchanged program and Rust source hashes, matched linking modes, and complete output for every run. `--imre-build-record` identifies the frozen compiler/runtime and their exact sources; use `--help` for the required artifact and baseline record paths. Profiling remains separate from wall-time measurement.

The [exception-performance follow-up](results/2026-10-18-exception-performance.md) reduces large all-changed ThinLTO control times by 7.6% / 12.4% and large batch times by 8.4–14.5%. The controls still take 4.6% / 6.8% more time than the pre-exception build. The report includes ordinary-link results, unchanged Rust comparisons, all raw samples, isolated instruction-profile stages, and recovery/ownership validation.

## File-manifest planner

The [manifest planner](../examples/manifest_planner/README.md) uses typed UTF-8 files, `with` cleanup, shared process arguments, scalar formatting, and line splitting. It reuses the batch payload parser and incremental graph algorithm, recovers from input failures, and writes every result to a report file. `manifest_validate.py` checks full directory snapshots with an independent parser and graph oracle; the Rust BigInt reference includes explicit checked file closure.

`manifest_benchmark.py` compares CPython, Imre, and Rust on six valid/mixed file workloads, and separately checks five unchanged incremental workloads against the preserved pre-file Imre executable. Both groups use 30 rounds with all six engine orders repeated five times, plus two warmups. Every execution gets a fresh isolated working directory; fixture creation, verification, and deletion are outside the clock. Startup, imports, file operations, parsing, planning, complete report output, close, and exit are timed. Inputs are page-cache warm, and writes are not fsynced. The example README documents frozen build records, baseline artifacts, linking modes, and reproduction.

The [recorded file-workload comparison](results/2026-10-18-manifest-planner.md) measures 1.86–3.27× faster execution than CPython with ThinLTO on large inputs, taking 1.72–1.79× Rust BigInt's time. Unchanged-program medians remain within 1% of the preserved exception-optimized build. The report includes all raw samples, an independent audit, exact source/build records, and complete native/sanitizer validation.

## Inventory reconciliation

The [inventory reconciler](../examples/inventory_reconciliation/README.md) uses eager list, dictionary, and set comprehensions to parse batched orders, select positive requests, fulfill them against shared inventory, and build complete reports. `inventory_validate.py` checks independent expected results and filesystem effects against CPython, Imre, and the [Rust BigInt reference](reference/inventory_reconciliation/README.md).

`inventory_benchmark.py` compares six inventory workloads and optionally reruns the five unchanged incremental-planner inputs from the concrete-layout comparison. Three-engine inventory runs default to 30 measured rounds covering all six engine orders five times, plus two warmups. The timer includes process startup, input, reconciliation, complete report output, flush/close, and exit; fixture preparation and verification happen outside it. Both ordinary and matched ThinLTO runs retain frozen source/build identities and every raw sample. The package guide gives reproduction commands and the shared input domain.

To compare a native optimization, preserve the old inventory executable and its JSON record, then add `--inventory-baseline PATH --inventory-baseline-record ORIGINAL.json` to the benchmark command. The inventory group includes CPython, preserved Imre, current Imre, and the same Rust reference; it reuses the original record's exact input bytes and independently verifies the expected reports. Defaults become 48 measured rounds, covering all 24 orders twice, with two warmups. An explicit `--samples` must be a multiple of 24. Application-source and preserved-executable hashes are checked against the original record; the current target, linking mode, Clang, and linker must match the baseline. The new compiler/runtime have their own frozen build record. The separate `--baseline` and `--baseline-record` options still select unchanged incremental-planner controls.

When intentionally changing application syntax, also pass `--inventory-baseline-source-root SNAPSHOT`, where `SNAPSHOT/inventory_reconciliation` preserves the old package. The complete Python file set and every source hash must match the baseline record. This explicit option permits source changes and records the old/current hashes and exact changed-file list; without it, identical application sources remain required. Snapshot files are protected from output overwrites and checked again after the comparison. CPython and current Imre run the current package; the preserved executable runs the old package. All engines still pass the full independent oracle and every workload output/filesystem check. Report any source changes alongside the timing results.

`inventory_profile.py` profiles saved executables without rebuilding them. After producing a comparison and retaining its artifacts, run:

```sh
python3 benchmarks/inventory_profile.py \
  --record benchmarks/local/inventory-comparison.json \
  --output benchmarks/local/inventory-profile \
  --cpu 0
```

Defaults run Callgrind and DHAT separately for Imre and Rust on the captured `dense-16384` and `sparse-16384` cases. Select engines or cases with repeated `--engine` or `--workload` options. For the preserved inventory executable, add `--engine baseline --baseline-record ORIGINAL.json`; include `--engine imre --engine rust_bigint` to profile all three native implementations. Binary, IR, build-record, and source identities are checked before and after profiling, with complete output and filesystem validation in fresh directories. A new output directory retains the captured cases, reports, source/build snapshots, raw traces/logs, hashes, and summaries. The driver refuses ptrace, records the host's counter-access setting, and accepts `--valgrind`/`--valgrind-lib` overrides. Callgrind counts instructions, not hardware cycles or elapsed-time shares; DHAT reports requested heap traffic and live allocations, not RSS. Run normal timing separately after all profiling and builds finish.

The [recorded comprehension milestone](results/2026-10-18-comprehensions.md) includes ordinary and ThinLTO comparisons, all 1,980 measured executions, an independent audit, and native/sanitizer validation. With matched ThinLTO, the two 16,384-positive-request inventory workloads run 1.79–2.24× faster than CPython and take 1.56–1.57× Rust BigInt's time. The unchanged incremental controls retain byte-identical generated LLVM IR and show median changes below 0.9% with overlapping interquartile ranges.

The [scalar f-string milestone](results/2026-10-18-scalar-fstrings.md) converts the inventory renderer while preserving byte-identical LLVM and native `.text` in both linking modes. All 2,304 measured executions and the unchanged report semantics are checked against CPython, Rust, and the previous optimized Imre executable. The report records the syntax boundary, source change, validation evidence, and full timing comparison; it makes no native speedup claim.

The [keyword-argument milestone](results/2026-10-19-keyword-arguments.md) adopts named calls, keyword-only parameters, and scalar defaults while retaining identical LLVM and native `.text` in both modes. ThinLTO Imre runs the two large inventory cases 2.71–3.60× faster than CPython and takes 4.6–9.8% more time than Rust BigInt in this run. The report preserves all 2,304 measured executions, the old-source baseline snapshot, rebuilt byte-identical Rust references, and validation evidence; it makes no native speedup claim.

The [conditional-expression and sequence-access milestone](results/2026-10-18-sequence-access.md) adopts conditional expressions, Unicode string indexing/iteration, and list/string read slicing in the inventory example. ThinLTO runs the large cases 2.68–3.60× faster than CPython and takes 5.0–10.5% more time than Rust BigInt. The report separates changed-source primary comparisons from paired native controls using the exact previous package, retains all 3,456 measured executions, and adds boundary/Unicode parser differential checks and full ASan/LSan inventory validation. All primary and control interquartile ranges overlap their preserved baselines; this is a capability milestone without a native speedup claim.

The [floats, bytes, matching, and assignment-expression milestone](results/2026-10-19-floats-bytes-match-walrus.md) adds the four capabilities and exercises them together in a binary measurement parser. Its inventory comparison holds all six application files unchanged and reruns CPython, preserved/current Imre, and Rust BigInt in both linking modes. ThinLTO runs the two large cases 2.70–3.56× faster than CPython and takes 6.1–10.1% more time than Rust. The report retains all 2,304 measured executions, source/build records, an independent timing audit, and the final semantic and sanitizer checks. These inventory measurements assess existing-program performance after the compiler/runtime changes; they do not measure throughput of the new features.

The [truthiness and loop-else milestone](results/2026-10-19-truthiness-loop-else.md) adopts Python truth tests, operand-returning `and`/`or`, and `for`/`while` `else` in the inventory reconciler. ThinLTO runs the large cases 2.75–3.61× faster than CPython and takes 4.1–8.7% more time than Rust BigInt. Four comparison families separately measure the updated application and the unchanged previous source in ordinary and ThinLTO builds, retaining all 4,608 measured executions. All unchanged-source interquartile ranges overlap their preserved baselines. The report includes independently audited timings, source/build provenance, 1,654 language checks, and inventory ASan/LSan validation.

The [assignment, unpacking, and expansion milestone](results/2026-10-19-assignment-unpacking-expansion.md) adopts chained assignment, starred `sys.argv` unpacking, row unpacking, and dictionary-key expansion in the inventory reconciler. ThinLTO runs the large cases 2.71–3.63× faster than CPython and takes 1.9–9.7% more time than Rust BigInt. Four families compare the updated application and unchanged previous source under ordinary and ThinLTO linking, retaining all 4,608 measured executions. The report includes the preserved-Imre comparisons, sample spreads, independently audited provenance, 2,417 semantic checks, 92 Rust tests, and inventory ASan/LSan validation. These measurements assess the compatibility milestone; they do not establish an isolated syntax cost or a native speedup.

## ty memory-report comparison

The [memory-report example](../examples/memory_report/README.md) ports the real
file and directory comparator from Ruff's `scripts/memory_report.py`. It decodes captured ty
JSON into typed records, ranks memory changes, and writes a complete Markdown
report. `memory_validate.py` checks the unchanged upstream script, typed CPython
port, Imre, and Rust against an independent oracle, including strict JSON
failures and filesystem effects. Only upstream's equal-change tie order may vary.

`memory_benchmark.py` measures the full process, including directory scans when
selected, complete file reads, JSON decoding, comparison, formatting, and report
writing. It uses genuine ty captures, explicitly labeled larger copies, many-small
and few-large file layouts, balanced four-engine orders, and an empty-report
control. Separate compute entrypoints preload JSON text and repeat decoding,
comparison, and rendering, with complete output checks and a zero-repetition
preload control. Compute timings compare Imre, upstream CPython, and Rust with
balanced three-engine orders. Upstream also echoes its report to stderr in the
ordinary application; that work is included and checked.
The [Rust reference guide](reference/memory_report/README.md)
documents reproduction commands and matched ordinary/ThinLTO builds.

The [memory-report results](results/2026-10-21-memory-report.md) meet the ≤2× Rust
target in both builds: 1.13–1.46× on 65,024 items per input file. They retain all
840 final benchmark executions, the exploratory baseline, CPU/allocation
profiles, complete output checks, and a portable evidence audit. Empty controls
are 1.60–2.75% of the large native medians; small genuine captures are
startup-heavy.

The [dataclass milestone](results/2026-10-21-dataclass-records.md) replaces handwritten
record constructors with dataclasses, adds a class method returning `Self`, and
uses a read-only property and frozen difference records. The large cases take
1.16–1.47× Rust's time and run 2.39–2.83× faster than the unchanged upstream
CPython script. Both build modes preserve complete reports and typed JSON
semantics. The report retains all samples, independent output/provenance checks,
sanitizer validation, and a portable evidence replay.

The [native-path milestone](results/2026-10-21-native-paths.md) adds directory
comparison through native `pathlib.Path`. Across ordinary and ThinLTO builds,
complete runs take 0.91–1.28× Rust's time and run 2.13–2.89× faster than the
unchanged upstream CPython script. Separate repeated parse/compare/render runs
take 1.38–1.63× Rust's time. The two layouts contain 65,024 items per side,
distributed over 256 small projects or four larger projects. All 1,344 measured
executions, validation results, and source bindings are independently audited
and retained in portable evidence.

`memory_process_benchmark.py` extends this workflow to native process orchestration.
It runs both pinned ty versions over retained inventory/Ruff source snapshots,
then compares 64 small captures with two large captures through an identical native
child producer. Every layout has 16,256 records per side. The timed Python baseline
is explicitly a prepared-project adapter using upstream's unchanged models and
renderer and matching atomic output; the typed CPython port is also validated.
Both layouts run with and without persisted captures in fresh numeric project
directories. Complete child execution, UTF-8 stdout/stderr capture, per-file
staging/replacement, final atomic report publication, and workspace cleanup are
included, with separate two-child empty-report controls. Capture trees are reset
outside timing. The comparison baseline is also an explicit atomic-output adapter
around the unchanged upstream comparison CLI.
Directory comparisons of the same reports separate filesystem/report work from
child execution.
Separate repeated compute workloads and zero-repetition preload controls expose
report-processing performance without ty or process-launch costs. The default
matrix has 13 timed rows. A separate correctness-only tee records the same actual
ty invocation's raw streams before forwarding them, allowing exact validation of
persisted captures despite run-to-run memory/diagnostic variation; timed real-ty
runs still execute ty directly. See the
[reference guide](reference/memory_report/README.md) for preparation, measurement,
source pinning, and ordinary/ThinLTO commands. No host environment values are
copied into the process benchmark record.

The [workspace and atomic-output results](results/2026-10-21-native-workspaces.md)
cover all 13 workloads in ordinary and ThinLTO builds. Complete process runs take
1.00–1.57× Rust's time, file-only reports take 1.13–1.26×, and repeated computation
takes 1.42–1.57×. Every row, including controls, meets the ≤2× target. The report
retains all 3,744 measured executions, exact persisted-output checks, sanitizer
validation, and a portable replay of the evidence and statistics.

## Compute-heavy developer tools

### Live conformance workflow

The conformance reporter can also run directly from a typing checkout, including
typed TOML configuration, path resolution, two actual ty child processes with
timeouts, and atomic report publication. Its [application guide](../examples/conformance_report/README.md)
shows the live CLI and captured-input replay.

For this workflow, `live_conformance_build.py` builds both Imre executables from
a retained `native_tools_build.py` compiler/runtime record. The live benchmark
requires matching Imre-program, Rust, Codon, and native-platform build records,
and validates full reports and filesystem effects across all five engines before
timing. It measures changed and unchanged reports, plus four-round frozen-input
report computation and zero-round controls. The latter keep analyzer latency
separate from compiler performance.

```sh
python3.14 benchmarks/live_conformance_build.py \
  --core-build-record /path/to/core/build-record.json --mode normal \
  --clang clang-18 --output /path/to/new-imre-programs

python3.14 benchmarks/live_conformance_benchmark.py \
  --suite /path/to/pinned-typing/conformance \
  --old-ty /path/to/ty-0.0.60 --new-ty /path/to/ty-0.0.90 \
  --python /path/to/python-with-tomli \
  --imre /path/to/new-imre-programs/conformance-report \
  --imre-compute /path/to/new-imre-programs/conformance-report-compute \
  --codon /path/to/codon-build/conformance_report \
  --codon-compute /path/to/codon-build/conformance_report-compute \
  --rust /path/to/rust-build/conformance-report-reference \
  --build-record /path/to/new-imre-programs/build-record.json \
  --build-record /path/to/codon-build/build-record.json \
  --build-record /path/to/rust-build/build-record.json \
  --build-record /path/to/platform-build/build-record.json \
  --samples 20 --warmups 2 --output /path/to/new-measurements
```

Use the complete typing checkout pinned in the application guide and matching
ordinary or ThinLTO artifacts. For ThinLTO, select `--mode thinlto` and the explicit
`--lto-linker` when building Imre programs, and build the Rust reference and
platform with `--thinlto`. Keep the benchmark host free of builds and profilers
during measurement. Raw samples are retained incrementally; only a final passing
`benchmark.json` with all input guards represents a completed comparison.

Audit completed ordinary and ThinLTO records independently of the timing driver:

```sh
python3.14 benchmarks/live_conformance_audit.py \
  /path/to/ordinary-measurements/benchmark.json \
  /path/to/thinlto-measurements/benchmark.json
```

The audit checks raw sample ordering, complete outputs, compute checksums,
statistics, validation records, captured inputs, and retained build identities.
For a copied evidence bundle, use `--portable` and `--rebase OLD=NEW` to map
retained source and artifact directories; the receipt reports which evidence
categories were checked. Auditing does not execute or rebuild native programs.
The [checkout comparison](results/2026-10-21-live-conformance.md) contains the
five-engine timing tables, retained evidence, and a portable audit command.
The [fixed-regex comparison](results/2026-10-22-fixed-regex.md) measures the
annotation scanner separately and repeats the report and checkout workloads
against the previous Imre scanner, CPython, Codon, and Rust.

### Streaming Python download catalog

The catalog benchmark measures complete processes on the shipped release index
and on ten consecutive copies. Each scale has a plain NDJSON file and a gzip
file. The 10× archive contains ten concatenated members. All four inputs must
produce the same 6,098-entry catalog. A separate preflight gives every member
a unique winning download and compares the full result across engines.

The latest comparison includes full-process time and peak RSS for every engine,
paired runs against the previous Imre binaries, and profiles showing how many
record and string allocations remain. The studies include raw measurements and
replay commands.
[Record and string allocations](results/2026-10-22-catalog-ownership.md) ·
[Earlier compiler and decoder improvements](results/2026-10-22-catalog-hot-paths.md) ·
[Original streaming comparison](results/2026-10-22-streaming-catalog.md).

Build the compiler and runtimes with `native_tools_build.py` as described below.
For each Imre program, use `live_conformance_build.py --application python_catalog`
with the core build record and the matching `--mode` and linker. Build the
independent `reference/python_catalog` once per native mode, and build Codon
with `codon_native/build.py --applications python_catalog`.

```sh
python3.14 benchmarks/stream_catalog_benchmark.py prepare --directory /tmp/catalog-inputs
python3.14 benchmarks/stream_catalog_benchmark.py run \
  --inputs /tmp/catalog-inputs --directory /tmp/catalog-measurements \
  --codon /path/to/codon-build/python_catalog \
  --imre /path/to/imre-normal/python_catalog --imre-thin /path/to/imre-thin/python_catalog \
  --rust /path/to/rust-normal/python-catalog-reference --rust-thin /path/to/rust-thin/python-catalog-reference
```

The runner verifies the build record next to each native binary. On one pinned
Linux CPU it measures elapsed time and child peak RSS for each fresh process,
including decompression and final output. Fourteen counterbalanced orders share
the upstream CPython, typed CPython, and Codon baselines across the two native
build modes. Every output is compared byte for byte outside timing.

### Captured-input developer tools

The [conformance reporter](../examples/conformance_report/README.md) ports Ruff's
offline annotation and diagnostic comparison. The
[Python download catalog](../examples/python_catalog/README.md) ports uv's offline
CPython release selection and rendering. Both compare complete outputs with the
unchanged upstream algorithms through small input/output adapters. Their guides
describe input preparation, the typed adaptations, and the accepted input domain.

Build frozen compiler/runtime artifacts from a clean checkout:

```sh
python3.14 benchmarks/native_tools_build.py \
  --target-dir target/native-tools \
  --shared-build-dir /tmp/imre-native-tools-build-cache \
  --output benchmarks/local/native-tools-build
```

The builder retains ordinary, ThinLTO, and AddressSanitizer runtimes, exact Cargo
commands and artifact paths, source snapshots, and compiler identities. It uses
the repository's pinned toolchain by default; pass `--toolchain ohm` on an Ohm
devbox. The two Rust references have separate builders in
[`reference/conformance_report`](reference/conformance_report/build.py) and
[`reference/python_catalog`](reference/python_catalog/build.py). The
[Codon guide](codon_native/README.md) records its pinned native release and every
adapter, including its fixed-width integer and duplicate-key boundaries.

`native_tools_benchmark.py --prepare-only` compiles the applications, validates
all engines, and freezes the inputs before measurement. Use `--prepared-record`
for the timing phase, with no concurrent builds or profilers. Its 12 workloads
cover complete tools, four repetitions of their entire compute pipelines, and
zero-repetition preload controls. Twenty samples cover two balanced five-engine
order cycles. Imre and Rust use matched ordinary/ThinLTO settings; the same
native Codon configuration is reported separately in both modes. Upstream
CPython and the typed CPython port are separate baselines.

Every measured process checks the complete rendered output and filesystem
effects outside the timer. `native_tools_audit.py` independently reconstructs
commands and expected outputs and recomputes the statistics from retained
samples. `native_tools_profile.py` records Callgrind CPU instructions and DHAT
allocations separately from uninstrumented timings.

Package a passing audit with the retained sources, raw inputs, full reports,
validation records, and pinned Codon license. Keep the measured checkout frozen
through packaging; `--source-binding` can instead supply a previously recorded
mapping from original source paths to unchanged snapshot files.

```sh
python3.14 benchmarks/native_tools_audit.py \
  /tmp/native-tools-normal.json /tmp/native-tools-thinlto.json \
  --output /tmp/native-tools-audit.json
python3.14 benchmarks/native_tools_evidence.py package \
  --audit /tmp/native-tools-audit.json \
  --codon-license /path/to/codon-v0.20.4-LICENSE \
  --output-dir /tmp/native-tools-evidence
python3.14 -B /tmp/native-tools-evidence/native_tools_evidence.py replay \
  /tmp/native-tools-evidence --output /tmp/native-tools-replay.json
```

The archive directory is portable: replay needs CPython 3.14+, with assertions
enabled, and uses its indexed source/input bytes without the original checkout,
network, or native engines. It independently reconstructs the complete upstream
outputs and rechecks sample order, statistics, ratios, and ordinary/ThinLTO input
parity. Native binaries and toolchains are identified by their recorded hashes
and explicitly excluded from portable rehashing. Pass `--extra PATH` to retain
additional validation logs or profile evidence without changing measured records.

## Callback record pipeline

The [payout example](../examples/record_pipeline/README.md) selects records with
bound policy methods, returns a gross or net transform as a typed callable, and
composes lazy `map`/`filter` with a noncapturing lambda. `record_validate.py`
checks complete output and filesystem effects against an independent arithmetic
oracle, CPython, and the [checked-i64/BigInt Rust reference](reference/record_pipeline/README.md).

`record_benchmark.py` separates compilation/oracle preparation from timing. It
compares gross/net and full/selective filtering at 32 and 4,096 records, with
balanced three-engine orders, repeated queries, and an empty-input control.
The reference guide gives reproduction commands, timing boundaries, storage
differences and matched ordinary/ThinLTO settings.

The [callable milestone report](results/2026-10-21-callable-benchmarks.md) records
the compatibility boundary, validation, and ordinary/ThinLTO measurements.

## Telemetry comparison

The [telemetry comparator](../examples/telemetry_comparison/README.md) aligns two recordings, computes signed/absolute/squared integer error, and answers threshold queries with generator expressions, strict `zip`, `reversed`, `any`, and `all`. `telemetry_validate.py` compares complete output and filesystem effects against an independent indexed oracle, CPython, and an independent Rust implementation with checked native arithmetic and a BigInt fallback.

The [iterator optimization report](results/2026-10-21-iterator-performance.md) brings all four 4,096-record full scans below 2× Rust's time: 1.39–1.72× with matched ThinLTO and 1.45–1.80× with ordinary linking. ThinLTO runs these cases 27.7–46.3× faster than CPython. The Python package and Rust reference remain unchanged. The report retains full-scan and early-exit comparisons at three sizes, all 2,898 benchmark executions, CPU/allocation profiles, and independently audited correctness and sanitizer evidence.

`telemetry_benchmark.py` separates preparation and semantic validation from timing. It compares ordinary and matched ThinLTO builds on short, medium, and long traces, with both full scans and early exits. Each process loads and validates the recordings once, repeats the selected query, and writes the final result plus a checksum. All six engine orders are balanced across measured rounds; startup controls are reported separately without subtraction. The [Rust reference guide](reference/telemetry_comparison/README.md) documents the storage differences, exact timing boundary, build records, and reproduction commands.

## Unchanged Luhn checksum

The [Luhn optimization report](results/2026-10-20-luhn-performance.md) compares unchanged upstream code with CPython, the preserved Imre executable, and the independent streaming Rust reference. With ThinLTO, 4,096-digit calls take 1.21–1.35× Rust's time, improve 27.7–29.4× over the previous Imre build, and run 44.6–47.6× faster than CPython. Short 16-digit calls still take 2.77–4.11× Rust's time. Bulk conversion, scalar tuple sums, fused consumers, guarded native reductions, and bounded storage reuse remove the dominant intermediate allocations. The report retains ordinary and ThinLTO results, all 2,184 process executions, short/large profiles, and independent audits of the frozen build and full validation matrix. The [original compatibility report](results/2026-10-20-luhn.md) retains the earlier baseline.

The [Luhn example](../examples/luhn_checksum/README.md) compiles the unannotated `mmcloughlin/luhn` 0.2.0 source unchanged. Its four string APIs exercise inferred signatures, `list(map(int, text))`, integer `sum`, stepped slices, comprehensions, and `divmod`. `luhn_validate.py` runs the original ten tests and five doctest examples through native adapters, then checks every decimal digit in the selected CPython Unicode database, mixed digit families, invalid text and UTF-8, exact errors, and output-file preservation.

`luhn_benchmark.py` compares all four APIs at 16, 256, and 4,096 digits per call. Each process repeats calls over a configurable digit budget and validates the full final output plus an accumulated checksum. Engine order is balanced, and a separate empty-input control exposes startup and protocol overhead without subtracting it. The [independent Rust reference](reference/luhn_checksum/README.md) uses a streaming algorithm, a pinned Unicode 16 decimal table, and `black_box` on each repeated call. The harness requires matching ordinary or ThinLTO builds and records raw samples, source/tool/binary identities, and build provenance.

Add `--baseline` and `--baseline-record` to compare a preserved Imre executable on the same captured inputs. The four-engine comparison defaults to eight warmup rounds and twelve measured rounds, using four fixed orders that balance engine positions and directed adjacent pairs within each round. The [example guide](../examples/luhn_checksum/README.md) describes the identity checks and reproduction options.

## Unchanged Base45 codec

The [decoder optimization report](results/2026-10-19-base45-decoder.md) records the latest comparison. In sustained 64 MiB batches, ThinLTO decoding takes 3.57–4.61× Rust's time. A separate same-session comparison using 2 MiB batches measures a 10.43–15.10× speedup over the previous Imre executable. The upstream codec and Rust reference remain unchanged. Bulk dictionary gathers, guarded integer loops, and bounded list-buffer reuse remove the largest measured allocation and CPU costs; intermediate Python containers still leave a gap with Rust. The report retains all samples, correctness checks, CPU/allocation profiles, and independent audits.

The [owned string append report](results/2026-10-19-base45-string-reuse.md) adds a Rust reference and removes quadratic string copying without editing the upstream codec. Sustained ThinLTO encoding takes 1.54× Rust's time for 16 KiB inputs, 1.68× for 1 KiB, and 2.41× for 64 bytes; the large case is 44× faster than the previous Imre executable in a same-session comparison. The shorter three-engine batches put encoding 18–28× ahead of CPython. At that milestone, decoding took 27–32× Rust's time. The report retains 1,950 process results, all sample spreads, allocation/instruction profiles, source/build provenance, and independent audits.

The [initial Base45 report](results/2026-10-19-base45.md) preserves the first unchanged-library comparison: both linking modes, all 720 measured executions, upstream and differential validation, an independent audit, and allocation profiles. These historical measurements predate owned string append. The current compiler reuses uniquely owned string storage with geometric capacity growth, avoiding repeated whole-prefix copies in the encoder while preserving existing aliases. The vendored upstream codec remains unchanged.

The [python-base45 example](../examples/base45_codec/README.md) vendors the codec and tests unchanged at a pinned upstream commit. `base45_validate.py` runs all five upstream test methods through a native adapter, checks 323 validly typed native calls and 289 further CPython comparisons, and separately requires compile-time rejection of the two wrong-typed upstream calls. Results, exact error messages, explicit causes, and filesystem effects must agree. Shared lookup-table aliases and repeated recovery are covered by the example's additional checks.

The [independent Rust reference](reference/base45_codec/README.md) uses preallocated byte buffers and the same typed file protocol, fixed alphabet, repeated calls, and checksum. Its `build.py` defaults to the repository's selected Cargo/Rust toolchain and records exact source, compiler, flags, and executable identities; Ohm is optional. The Rust validator runs 295 checks, including four explicitly recorded Unicode `repr` message boundaries that check error metadata and unchanged output while allowing different character quoting. Imre matches CPython's exact messages for every case. Timed decoder inputs are valid ASCII Base45; the reference guide documents the diagnostic boundary and matched ThinLTO settings.

`base45_benchmark.py --rust PATH` compares Imre, CPython, and Rust for encoding, bytes decoding, and text decoding at 64, 1,024, and 16,384 decoded bytes per call. By default, each process represents 2 MiB of raw data across repeated calls; decoding consumes the corresponding 3 MiB of Base45 data. The exact measured executables pass validation before timing. Six warmup rounds cover all six engine orders once, and 24 measured rounds cover each order four times on one CPU. Without `--rust`, the harness alternates Imre and CPython order. It records every duration, output hash, source/tool identity, interpreter configuration, executable/LLVM hash, and the Rust build record.

Use `--native-only --operations encode decode-bytes decode-text` with `--rust` for a native comparison that retains CPython validation; select fewer operations when only one path is relevant. Increasing `--batch-bytes` reduces the proportion of process startup in each sample. Timings still include startup, imports where applicable, codec work, checksum, and one input/output file operation; fixture preparation and verification are excluded. The example guide gives three-engine, sustained native, ordinary-link, and ThinLTO reproduction instructions. Run measurements separately from builds and profiling, preserving a fresh artifact directory for each comparison.

The [iterator-pipeline comparison](results/2026-10-20-iterator-pipelines.md) records all 42 workload/linking conditions, balanced raw samples, startup controls, CPU and allocation profiles, and complete validation evidence. At 4,096 records with ThinLTO, full scans run 4.48–5.84× faster than CPython and take 8.46–13.44× Rust’s time; early exits run 12.81–19.40× faster than CPython and take 1.55–2.91× Rust’s time. These are whole-process measurements, including loading and alignment.
