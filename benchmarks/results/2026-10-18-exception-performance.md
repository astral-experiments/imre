> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Reducing successful-path exception overhead

Imre retains catchable exceptions while reducing the measured overhead on unchanged planner programs. With ThinLTO, the large all-changed layered/wide controls take **77.67 / 42.98 ms**, down **7.6% / 12.4%** from the original exception-enabled build. They remain **4.6% / 6.8%** slower than the preserved pre-exception build. Large batch workloads improve **8.4–14.5%**; Rust BigInt still takes less time.

The measured implementation is clean commit `5d2e517599a951376eb9e2c60dcb0befcb94fade`. The incremental and batch Python packages and the Rust reference sources are unchanged. This follows the [original exception/batch milestone](2026-10-18-batch-planner.md).

## Whole-process comparison

Each result is the median of 32 fresh processes, pinned with the harness to CPU 0, after two warmups per implementation/workload. The timer surrounds `subprocess.run`: startup, imports, pipes, parsing, whole-graph validation, affected-set traversal, scheduling, complete output, and exit are included. Batch runs also include framing, recoverable parse failures, and cleanup. Builds, oracle validation, warmups, output comparisons, and hashing are excluded. Traced timing is refused. Ordinary and ThinLTO sessions ran sequentially after all builds, tests, and profilers stopped.

Each four-engine group uses the Williams rows `A B D C`, `B C A D`, `C D B A`, `D A C B`, repeated eight times. Every engine occupies each position eight times, and each directed adjacent pair within a round occurs eight times. Transitions between rounds are not balanced. All 2,992 measured/warmup executions passed exact stdout, exit-status, and stderr checks. Full input/expected output appears once per workload; every raw execution retains time, status, stderr, stdout hash and byte length.

Control engines are CPython, pre-exception Imre, original exception-enabled Imre, and optimized Imre. Batch engines are CPython, original exception-enabled Imre, optimized Imre, and Rust BigInt. The five control and six batch inputs are byte-identical to the prior recorded workloads. Large batches contain two valid 16,384-target requests; mixed variants additionally contain recoverable malformed frames. The 128-target boundary case stresses integer boundaries. These are whole-program comparisons, not direct timing of an exception operation.

All times below are milliseconds. `Imre [Q1–Q3]` shows the median and observed interquartile range; this spread is not a confidence interval. Reduction is `1 − optimized / original-exceptions`; residual is `optimized / pre-exceptions − 1`.

### Unchanged single-request control

**Ordinary linking**

| Input | CPython | Pre-exceptions | Original exceptions | Imre [Q1–Q3] | Reduction | Residual |
|---|---:|---:|---:|---:|---:|---:|
| layered-16384-sparse | 117.44 | 57.56 | 60.82 | 59.84 [58.90–60.56] | 1.6% | +4.0% |
| layered-16384-all | 198.46 | 85.99 | 92.31 | 89.79 [88.44–90.10] | 2.7% | +4.4% |
| wide-16384-sparse | 105.76 | 28.66 | 31.11 | 29.65 [29.46–29.89] | 4.7% | +3.4% |
| wide-16384-all | 189.03 | 49.66 | 54.78 | 51.59 [51.06–51.95] | 5.8% | +3.9% |
| boundary-128-all | 21.49 | 3.67 | 3.68 | 3.65 [3.63–3.70] | 0.9% | -0.5% |

**ThinLTO**

| Input | CPython | Pre-exceptions | Original exceptions | Imre [Q1–Q3] | Reduction | Residual |
|---|---:|---:|---:|---:|---:|---:|
| layered-16384-sparse | 117.03 | 50.30 | 54.90 | 52.23 [51.97–52.97] | 4.9% | +3.8% |
| layered-16384-all | 196.73 | 74.25 | 84.08 | 77.67 [76.62–78.76] | 7.6% | +4.6% |
| wide-16384-sparse | 105.84 | 23.58 | 27.24 | 24.48 [24.26–24.77] | 10.1% | +3.8% |
| wide-16384-all | 190.42 | 40.25 | 49.07 | 42.98 [42.18–43.35] | 12.4% | +6.8% |
| boundary-128-all | 22.04 | 3.71 | 3.85 | 3.74 [3.69–3.81] | 2.9% | +0.6% |

### Batch requests

`Imre/Rust` is elapsed-time ratio; values above one mean Rust is faster. `CPython/Imre` is the corresponding speed ratio.

**Ordinary linking**

| Input | CPython | Original exceptions | Imre [Q1–Q3] | Rust BigInt | Reduction | CPython/Imre | Imre/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| layered-16384-all-valid | 369.88 | 171.55 | 166.21 [163.92–169.00] | 101.98 | 3.1% | 2.23× | 1.63× |
| layered-16384-all-mixed | 370.38 | 171.99 | 165.72 [164.12–167.57] | 103.37 | 3.6% | 2.23× | 1.60× |
| wide-16384-all-valid | 361.88 | 101.91 | 94.78 [94.37–96.65] | 54.68 | 7.0% | 3.82× | 1.73× |
| wide-16384-all-mixed | 365.40 | 102.99 | 96.20 [94.80–98.03] | 55.33 | 6.6% | 3.80× | 1.74× |
| boundary-128-all-valid | 23.22 | 4.23 | 4.18 [4.15–4.24] | 3.77 | 1.2% | 5.56× | 1.11× |
| boundary-128-all-mixed | 23.29 | 4.31 | 4.29 [4.25–4.37] | 3.79 | 0.5% | 5.42× | 1.13× |

**ThinLTO**

| Input | CPython | Original exceptions | Imre [Q1–Q3] | Rust BigInt | Reduction | CPython/Imre | Imre/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| layered-16384-all-valid | 369.51 | 156.78 | 143.17 [141.85–143.73] | 92.30 | 8.7% | 2.58× | 1.55× |
| layered-16384-all-mixed | 366.87 | 156.04 | 142.91 [141.84–144.24] | 91.93 | 8.4% | 2.57× | 1.55× |
| wide-16384-all-valid | 359.01 | 89.42 | 76.82 [76.22–77.17] | 49.04 | 14.1% | 4.67× | 1.57× |
| wide-16384-all-mixed | 359.64 | 90.10 | 77.01 [75.92–78.23] | 49.63 | 14.5% | 4.67× | 1.55× |
| boundary-128-all-valid | 22.94 | 4.06 | 4.00 [3.96–4.04] | 3.72 | 1.3% | 5.73× | 1.08× |
| boundary-128-all-mixed | 23.10 | 4.19 | 4.11 [4.05–4.16] | 3.73 | 1.9% | 5.62× | 1.10× |

Small boundary-case differences overlap the observed spread and should not be treated as robust gains. No claim about C performance follows from these Rust comparisons.

## Profiling and changes

Callgrind profiles use the same captured 16,384-target all-changed controls. They count executed instructions, **not CPU cycles or percentages of wall time**. Hardware-counter access was denied (`perf_event_paranoid=4`); the denial is retained in the profile evidence. Each stage has one capture per workload with fresh default hash seeds. Inclusive call costs overlap; very small differences are not attributed to an optimization.

Four changes account for the selected implementation: a separate non-Drop TLS `Cell<bool>` makes the pending-error presence query cheaper; conservative effects/fixed-field proofs remove checks that cannot fail; error construction is cold and kept out of hot success paths, with error branches weighted 1:2000; private integer-list getters/pop return an impossible raw-zero failure sentinel and setters/copy return status, avoiding a separate TLS query. The pending error owner remains responsible for diagnostics and propagation. The integer representation, ownership, current bounds checks, and observable exception behavior remain intact.

The original ordinary build executes 2,417,728 / 2,621,829 pending queries on layered/wide controls, at about 13 exclusive instructions each. The optimized build executes 679,916 / 524,312, at five each; emitted query sites fall from 172 to 72. ThinLTO inlines the direct query and leaves 32,770 outlined presence-helper calls per control. Cold outlining also shrinks the successful `list_get_index` stack reservation from 104 to 40 bytes and removes two register saves; saved disassembly supports that mechanism.

Instruction counts below are millions, rounded only for display. The two middle columns are ordinary linking; the last two are ThinLTO. “Effects” and “flag” are independently measured first, then combined. Later rows are cumulative unless stated otherwise.

| Stage | Ordinary layered | Ordinary wide | ThinLTO layered | ThinLTO wide |
|---|---:|---:|---:|---:|
| Pre-exceptions | 475.731 | 301.514 | 370.549 | 234.982 |
| Original exceptions | 516.798 | 346.293 | 473.744 | 319.090 |
| Presence flag only | 497.454 | 325.280 | 443.499 | 281.675 |
| Compiler effects only | 511.241 | 342.426 | 466.715 | 314.437 |
| Flag + effects | 494.971 | 323.653 | 441.093 | 280.398 |
| + branch weights only | 494.700 | 323.429 | 435.959 | 276.090 |
| + cold errors only | 495.013 | 323.695 | 429.127 | 269.202 |
| + cold errors + weights | 494.710 | 323.431 | 418.200 | 261.924 |
| + integer-list result ABI | 487.388 | 313.876 | 415.529 | 261.957 |
| Exact measured final binaries | 487.353 | 313.846 | 415.581 | 261.966 |

The integer-list ABI adds a 1.5% / 3.0% ordinary instruction reduction beyond cold errors plus weights. Its ThinLTO effect is 0.6% on layered and effectively neutral on wide; it was retained for the ordinary-linking benefit. These ablations do not assign elapsed-time percentages to individual changes. The final ordinary archive was built in the workspace dependency context rather than the isolated runtime-only context; its distinct hash and an additional profile are preserved. Final profiles use the exact measured executables, not reconstructed substitutes.

The final ordinary controls execute 487,352,765 / 313,845,996 instructions; ThinLTO executes 415,580,807 / 261,965,707. Hashing/mapping operations, scheduling, string splitting, and allocation remain substantial instruction costs. Their presence identifies remaining work but does not establish a wall-time bottleneck or explain the Rust gap by itself.

## Toolchain, validation, and limits

The host is Linux x86-64 on an AMD EPYC-Milan devbox. CPython is 3.14.5 with the GIL enabled and JIT disabled; recorded build configuration indicates PGO and LTO. Imre uses Ruff/ty crates 0.0.14, Clang 18.1.3, and Rust 1.98.1-dev (`f62703110`, Ohm `ohm-1.98.1-1`, LLVM 22.1.8). Ordinary Rust references use their recorded no-LTO release profile. Both native ThinLTO builds use LLVM import/inline budgets of 1000; Rust and Imre still differ in frontend and backend details. The Rust BigInt reference is a direct port with the same algorithm and arbitrary-precision numeric domain, not generated from the Python source.

Every control engine passed 155 independent graph-oracle inputs and 17 parser cases; every batch engine passed 194 inputs, covering 393 framed requests, 342 valid parses, 51 recovered parse failures, and nine terminal framing cases. The graph oracle uses independent topology/closure/scheduling logic, and the batch oracle independently parses bounded frames. All expected output is checked in full.

The [validation record](2026-10-18-exception-performance-validation.json) contains ordinary O0/O3, ThinLTO O3, and generated-code plus Rust-runtime ASan O1 results for exceptions, containers, integers, imports, classes, and I/O. It records 48 ordinary exception checks and 32 each for ThinLTO/ASan, all 194 batch inputs under ASan, 28 runtime unit tests, 17 compiler unit tests, formatting, and Clippy. Leak detection was enabled outside ptrace; the prebuilt Rust standard library was not instrumented. The isolated ordinary archive used for broad suites is source-identical to the final workspace archive; the final archive separately passed the full benchmark oracles and 48 exception checks.

The [recovery-memory record](2026-10-18-exception-performance-recovery-memory.json) uses one executable for 800 and 80,000 caught failures. Both runs peak at 18,140 requested live bytes in 14 blocks and end with 17,408 bytes in three persistent I/O buffers. This is DHAT requested heap, not RSS, and evidence for these recovery paths rather than a general leak-freedom proof. The stress source and complete normalized traces are embedded.

AOT compilation is excluded from timings. Control/batch compile times were 0.615 / 0.721 s ordinary and 5.006 / 5.479 s ThinLTO. Compiler/runtime builds are excluded from those figures. Ordinary control/batch executables are 3,152,616 / 3,156,768 bytes; ThinLTO executables are 2,752,968 / 2,847,960 bytes. LLVM IR is 168,190 / 255,476 bytes.

This is a shared devbox: affinity and balanced order reduce some noise, but frequency changes and shared resources remain uncontrolled. Fresh-process startup matters, especially at 128 targets. The baseline delta is the combined compiler/runtime change, not the isolated cost of a Python exception statement. Imre supports a narrower exception contract without CPython tracebacks, context, or chaining; mixed-batch comparisons cover the supported observable behavior, not identical exception internals. Results apply to these programs, inputs, and toolchains.

## Evidence and reproduction

- [Ordinary raw comparison JSON, gzip](2026-10-18-exception-performance-normal.json.gz) and [ThinLTO raw comparison JSON, gzip](2026-10-18-exception-performance-thinlto.json.gz): unchanged raw bytes after decompression, every sample and expected output.
- [Raw-data audit](2026-10-18-exception-performance-audit.json), [profile evidence and source reconstruction patches](2026-10-18-exception-performance-profiles.json), and [frozen compiler/runtime build record](2026-10-18-exception-performance-build.json).
- [Sanitizer build record](2026-10-18-exception-performance-sanitizer-build.json), [validation](2026-10-18-exception-performance-validation.json), and [recovery memory](2026-10-18-exception-performance-recovery-memory.json).

Compression uses gzip with `mtime=0`; local original JSONs were retained. The decompressed SHA256 values are `ba2598ac3e9d868a6007d51e8840846f2b07e975c95f337494a83482451f4e20` (ordinary) and `6f86a575f994ece70a17dcb63744114e46af2b5d29a57e67e33bc1518cd26dc1` (ThinLTO). For inspection:

```sh
gzip -dc benchmarks/results/2026-10-18-exception-performance-normal.json.gz > /tmp/exception-normal.json
gzip -dc benchmarks/results/2026-10-18-exception-performance-thinlto.json.gz > /tmp/exception-thinlto.json
sha256sum /tmp/exception-normal.json /tmp/exception-thinlto.json
```

Build at the recorded revision with the repository toolchain (`cargo build --release --workspace --locked`), or locally with `cargo +ohm -Zohm-defaults=no build --release --workspace --locked` when required. A ThinLTO runtime uses `RUSTFLAGS=-Clinker-plugin-lto` and a separate target directory. Preserve the validated pre-exception and original-exception binaries and their comparison records from the prior milestones, plus a current compiler/runtime build record and matching Rust batch build record. The harness verifies their real hashes; rebuilding a baseline requires producing a new baseline record, not changing hashes in an old record. Build commands, source snapshots, compiler versions, and archive hashes are preserved in the linked build records.

With those artifacts, the ordinary comparison is:

```sh
python3 benchmarks/exception_benchmark.py \
  --compiler "$TOOLS/imre" --runtime "$TOOLS/libimre_runtime.a" \
  --imre-build-record "$TOOLS/build-record.json" \
  --pre-exception-control "$PRE/incremental_planner" \
  --pre-exception-record "$PRE_RECORD" \
  --baseline-control "$POST/incremental_planner" \
  --baseline-batch "$POST/batch_planner" --baseline-record "$POST_RECORD" \
  --rust-bigint "$RUST/planner-batch-bigint" \
  --rust-build-record "$RUST/build-record.json" \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --samples 32 --warmups 2 --cpu 0 \
  --artifact-dir benchmarks/local/exception-reproduction \
  --output benchmarks/local/exception-reproduction.json
```

`TOOLS`, `PRE`, `POST`, and `RUST` denote the corresponding local artifact directories; `PRE_RECORD` and `POST_RECORD` denote their original comparison records. For ThinLTO, choose all matching ThinLTO artifacts/records, use `libimre_runtime_lto.a`, and add `--lto-linker /path/to/ld.lld` with an actual compatible Rust LLD backend symlink. Preserve the lexical `ld.lld` name; do not canonicalize it to `rust-lld`. Build the Rust reference with recorded ThinLTO and `--llvm-budget 1000` controls as described in its [README](../reference/incremental_planner/README.md). Run outside ptrace after other work stops.

For profiling, extract the two all-changed control inputs from a comparison record, add `shape` (`layered-all`/`wide-all`) and `tasks=16384`, and retain their input/output hashes. Then use `dependency_profile.py --record INPUT_RECORD --binary final=BINARY --build-record final=COMPARISON_RECORD --output NEW_DIRECTORY --tools callgrind --cpu 0`. The profile artifact retains exact commands, trace/log/stdout hashes, raw-summary hashes, and source edits that reconstruct every intermediate snapshot against the published original build record. Never run profiling concurrently with timing.

The memory experiment is reproducible without temporary local scripts: extract `input_record.source` from the recovery-memory JSON, compile it at ordinary O3 with the recorded tools, and run the same executable under `valgrind --tool=dhat` with stdin `100` and `10000`. Compare complete output with CPython and inspect DHAT peak/end live bytes and blocks; do not interpret instrumented elapsed times as speed results.
