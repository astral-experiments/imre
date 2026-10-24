> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Optimizing unchanged Luhn toward Rust performance

Imre compiles the original, unannotated [`mmcloughlin/luhn` 0.2.0](https://github.com/mmcloughlin/luhn/tree/d6f3fa71072f99334a4c1d2a3f062fa93982797f) without rewriting its source. This change removes the dominant temporary objects and checked arithmetic from its hot paths through general compiler and runtime optimizations.

With ThinLTO, **4,096-digit calls take 1.21–1.35× Rust's time**, or 21–35% more. They run **27.7–29.4× faster than the preserved Imre build** and 44.6–47.6× faster than CPython.

Short-call Rust parity remains open: 16-digit calls take **2.77–4.11× Rust's time**; 256-digit calls take 1.35–1.59×. Ordinary linking takes 1.44–1.61× Rust's time at 4,096 digits and 3.77–6.10× at 16 digits.

The [implementation PR](https://github.com/viarius-experiments/imre/pull/55) remains in the existing GitHub stack. The upstream Python module, typed driver, upstream tests and license, and independent Rust implementation are unchanged. The previous [compatibility report](2026-10-20-luhn.md) retains the original measurements; the comparison here reruns its preserved Imre binaries alongside the new build on the same inputs.

## Wall-time comparison

Each workload process handles **16 MiB of ASCII digits** through repeated calls at the indicated input size. Each build family uses eight warmup rounds and twelve measured rounds. Four fixed engine orders balance positions and directed adjacent pairs within each round; transitions between rounds are not balanced.

Values are median milliseconds with inclusive quartiles in brackets. **Previous / current** and **CPython / Imre** are speedups, so higher is better. **Imre / Rust** is a time ratio, so lower is better. Quartiles describe the observed samples, not confidence intervals.

### Ordinary linking

| API / digits | Imre ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Previous Imre ms | Previous / current | Imre / Rust | CPython / Imre |
|---|---:|---:|---:|---:|---:|---:|---:|
| `checksum` / 16 | 105.330 [104.615–105.706] | 27.918 [27.609–28.696] | 2082.708 [2072.469–2111.350] | 1876.665 | 17.817× | 3.773× | 19.773× |
| `verify` / 16 | 128.170 [127.796–128.703] | 29.069 [28.964–29.197] | 2119.008 [2111.875–2129.423] | 1870.014 | 14.590× | 4.409× | 16.533× |
| `generate` / 16 | 179.193 [178.919–179.590] | 29.364 [29.167–29.519] | 2194.524 [2190.616–2200.956] | 2048.852 | 11.434× | 6.102× | 12.247× |
| `append` / 16 | 234.361 [233.681–236.843] | 57.718 [57.445–58.911] | 2339.687 [2333.214–2346.266] | 2107.666 | 8.993× | 4.060× | 9.983× |
| `checksum` / 256 | 42.594 [42.444–42.882] | 24.937 [24.747–25.160] | 1557.162 [1547.180–1563.334] | 1369.072 | 32.142× | 1.708× | 36.558× |
| `verify` / 256 | 43.307 [42.867–43.589] | 24.866 [24.826–24.935] | 1558.860 [1555.700–1565.072] | 1380.958 | 31.887× | 1.742× | 35.995× |
| `generate` / 256 | 47.974 [47.851–48.105] | 24.846 [24.600–24.921] | 1559.862 [1550.605–1565.772] | 1389.511 | 28.964× | 1.931× | 32.515× |
| `append` / 256 | 54.061 [53.900–54.534] | 28.395 [28.310–29.220] | 1587.508 [1573.879–1623.941] | 1410.240 | 26.086× | 1.904× | 29.365× |
| `checksum` / 4,096 | 38.752 [38.399–38.919] | 24.378 [24.282–24.514] | 1456.735 [1450.331–1471.253] | 1331.156 | 34.351× | 1.590× | 37.592× |
| `verify` / 4,096 | 38.474 [38.254–38.663] | 24.480 [24.234–24.626] | 1449.089 [1437.863–1458.236] | 1327.581 | 34.506× | 1.572× | 37.664× |
| `generate` / 4,096 | 39.153 [39.000–39.346] | 24.346 [24.262–24.383] | 1461.249 [1454.002–1467.847] | 1326.420 | 33.878× | 1.608× | 37.321× |
| `append` / 4,096 | 40.885 [40.756–41.027] | 28.341 [28.221–28.573] | 1455.665 [1449.648–1479.863] | 1328.641 | 32.497× | 1.443× | 35.604× |

### ThinLTO

| API / digits | Imre ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Previous Imre ms | Previous / current | Imre / Rust | CPython / Imre |
|---|---:|---:|---:|---:|---:|---:|---:|
| `checksum` / 16 | 75.905 [75.148–76.243] | 25.640 [25.580–25.709] | 2072.757 [2064.871–2080.538] | 1245.621 | 16.410× | 2.960× | 27.307× |
| `verify` / 16 | 74.961 [74.473–76.201] | 27.101 [27.048–27.143] | 2110.346 [2096.037–2118.328] | 1251.026 | 16.689× | 2.766× | 28.153× |
| `generate` / 16 | 109.410 [107.412–110.911] | 26.599 [26.470–26.719] | 2195.383 [2185.292–2213.908] | 1355.433 | 12.389× | 4.113× | 20.066× |
| `append` / 16 | 155.530 [154.399–156.160] | 50.472 [50.022–51.232] | 2336.596 [2325.406–2347.511] | 1441.935 | 9.271× | 3.082× | 15.023× |
| `checksum` / 256 | 34.015 [33.838–34.593] | 23.203 [23.078–23.374] | 1559.131 [1558.643–1563.844] | 946.384 | 27.822× | 1.466× | 45.836× |
| `verify` / 256 | 33.886 [33.750–34.075] | 25.077 [24.859–25.479] | 1563.747 [1559.257–1583.221] | 949.854 | 28.031× | 1.351× | 46.148× |
| `generate` / 256 | 37.489 [37.295–37.625] | 24.012 [23.781–24.759] | 1571.243 [1567.913–1589.110] | 964.439 | 25.726× | 1.561× | 41.912× |
| `append` / 256 | 42.161 [41.843–42.375] | 26.437 [26.225–26.596] | 1578.058 [1573.966–1587.858] | 974.638 | 23.117× | 1.595× | 37.430× |
| `checksum` / 4,096 | 30.555 [30.422–30.645] | 22.588 [22.444–22.754] | 1446.536 [1442.599–1466.128] | 892.620 | 29.214× | 1.353× | 47.342× |
| `verify` / 4,096 | 30.480 [30.409–31.000] | 24.080 [24.036–24.266] | 1450.868 [1445.523–1460.398] | 896.313 | 29.406× | 1.266× | 47.600× |
| `generate` / 4,096 | 31.083 [30.918–31.319] | 23.092 [22.945–23.157] | 1451.285 [1448.828–1463.164] | 894.909 | 28.791× | 1.346× | 46.691× |
| `append` / 4,096 | 32.685 [32.574–33.024] | 26.910 [26.735–27.172] | 1458.973 [1452.936–1478.646] | 906.068 | 27.721× | 1.215× | 44.638× |

The [Rust reference](../reference/luhn_checksum/README.md) implements the same externally visible string operations, validation order, diagnostics, and file protocol. It streams over digits and uses a virtual trailing zero for `generate`, avoiding Python's intermediate digit list. Both engines receive identical inputs and repetition counts and must produce the same complete output and accumulated checksum. Rust uses a black-box barrier around each repeated call. These measurements compare complete implementations, including their different storage and algorithm choices.

Native engines use matching ordinary or ThinLTO configurations. ThinLTO uses matched LLVM import and inline budgets of 1,000. The build families ran separately with their own interleaved baselines; their difference does not isolate the effect of ThinLTO.

| Build family | Imre ms | Rust ms | CPython ms | Previous Imre ms |
|---|---:|---:|---:|---:|
| Ordinary | 2.366 | 2.356 | 26.136 | 2.671 |
| ThinLTO | 2.402 | 2.157 | 26.715 | 2.387 |

The empty-input, one-call controls expose startup and protocol overhead. They are retained without subtraction. Across both build families, the control is at most 7.88% of current Imre's median workload time, 9.68% of Rust's, 1.85% of CPython's, and 0.27% of the previous Imre build's.

## What changed

The original profile identified character construction, integer parsing, destruction, and temporary `divmod` pairs as the largest costs. The compiler and runtime now:

- Convert exact `list(map(int, text))` consumers directly into an integer list, with an ASCII fast path and the existing Unicode decimal and error behavior.
- Scalarize fixed tuple sums, including immediate `sum(divmod(...))`, and fuse eligible consumed slices and eager comprehensions while preserving evaluation, mutation, and exception order.
- Use closed-call-graph element bounds and a guarded reduction budget to select native accumulator arithmetic. The guarded loop can borrow exact cursor reads before their unique advance without repeated bounds checks. Other reads and uncertain cases retain checked fallbacks.
- Remove redundant ownership work when the old integer value is proved small, and reuse bounded private list buffers and empty headers after all element owners are released.

The accumulator proof directly selects native arithmetic without redundant LLVM assumptions that hide the recurrence. LLVM retains its ordinary transformation legality and profitability decisions. The result is not a Luhn intrinsic: the transformations apply to the corresponding typed IR patterns.

The approach follows useful ideas in scriptc's [array pipeline lowering](https://github.com/vercel-labs/scriptc/blob/dfb0bed8c1e4e387f6901ccb155d0d6b952f6c0a/packages/compiler/src/frontend/lowering/containers/array-pipelines.ts#L555), [guarded dense-array loops](https://github.com/vercel-labs/scriptc/blob/dfb0bed8c1e4e387f6901ccb155d0d6b952f6c0a/packages/compiler/src/backend/llvm/dense-array-loops.ts#L1), and [integer range analysis](https://github.com/vercel-labs/scriptc/blob/dfb0bed8c1e4e387f6901ccb155d0d6b952f6c0a/packages/compiler/src/ir/integer-ranges.ts#L11): expose concrete storage, eliminate unobservable intermediates, and supply facts at the point where they hold. Imre implements its own proofs in Rust and preserves the supported Python behavior.

Private storage reuse retains at most two empty buffers totaling 512 KiB and two empty headers totaling 80 bytes per thread on the supported 64-bit layout, excluding allocator/TLS bookkeeping. No element owner is retained in either cache. Aliased headers cannot be reused; cache reentry or thread-local teardown uses ordinary destruction.

## Allocation and instruction profiles

After wall timing, separate Callgrind and DHAT runs profiled the **exact timed ThinLTO binaries**. The short workload performs 65,536 checksum calls on 16 digits; the large workload performs 64 calls on 4,096 digits. Every run verifies the complete output and accumulated checksum against an independent ASCII arithmetic oracle.

| Workload | Engine | Allocated bytes | Allocation blocks | Peak live bytes | Guest instructions |
|---|---|---:|---:|---:|---:|
| 16 digits × 65,536 calls | Imre | 18,716 | 33 | 10,331 | 69,886,356 |
| 16 digits × 65,536 calls | Rust | 4,114 | 19 | 1,922 | 23,285,675 |
| 4,096 digits × 64 calls | Imre | 59,532 | 34 | 47,051 | 6,514,902 |
| 4,096 digits × 64 calls | Rust | 8,191 | 19 | 5,999 | 5,125,319 |

For the same 64 checksum calls on 4,096 digits, Imre now executes **6,514,902 guest instructions**, down from 212,067,563 in the preserved baseline: **a 32.55× reduction**. Requested heap allocation falls from 23,105,570 bytes in 395,039 blocks to **59,532 bytes in 34 blocks**—99.742% fewer bytes. The baseline executable matches the one in the final timing comparison; the Rust executable, input, repetition count, complete output, and accumulated checksum also match the original profile. These are whole-process Callgrind/DHAT measurements, including startup and I/O, not CPU-time percentages or elapsed-time estimates.

The remaining large-input instruction cost is concentrated in digit conversion and tagged storage (**38.24%** of process instructions), checksum code itself (**30.29%**), the strided subtotal (**17.80%**), and list release (**7.18%**). These are disjoint checksum call subtrees plus checksum self cost; the subtotal already includes normalization. The final checksum loop is **scalar**, executing 15 instructions per selected digit. Runtime conversion, the stride-two subtotal, and the immediate-value release scan execute SIMD instructions. On 16-digit inputs, conversion costs about 344 instructions per call, subtotal 266 (including 65 for normalization), release 169, and checksum self 232. Helper setup, ownership, and cache work matter much more at this size.

There are **no recurring character-string, divmod-pair, slice-list, or comprehension-result allocations** in either final checksum profile. The checksum allocation stacks contain one reusable digit buffer, one list header, and two TLS registrations over the entire process. The remaining allocation totals partition as follows:

| Requested allocation category | 16 digits × 65,536 calls | 4,096 digits × 64 calls |
| --- | ---: | ---: |
| Digit-list buffer | 128 B / 1 block | 32,768 B / 1 block |
| List header | 40 B / 1 block | 40 B / 1 block |
| Cache TLS registrations | 64 B / 2 blocks | 64 B / 2 blocks |
| Input bytes and decoded text | 56 B / 2 blocks | 8,232 B / 3 blocks |
| I/O buffers | 17,408 B / 3 blocks | 17,408 B / 3 blocks |
| Other startup and protocol work | 1,020 B / 24 blocks | 1,020 B / 24 blocks |
| **Total** | **18,716 B / 33 blocks** | **59,532 B / 34 blocks** |

These are whole-process guest instruction and requested allocation counts, not hardware cycles, CPU time, RSS, or latency predictions. Callgrind self costs and inclusive call-edge costs are reported separately to avoid double counting. DHAT cumulative allocation differs from peak live allocation. Profiles include startup, input decoding, file I/O, checksum accumulation, and cleanup. They directly attribute `checksum`; they do not by themselves establish the cause of other APIs' timing gaps.

For this checksum workload, removing more allocator calls is therefore no longer the main opportunity. The remaining work includes materializing tagged digits, reading them again for the subtotal and destruction scan, and scalar checksum arithmetic. Generic streaming/fusion or stronger value-proof propagation could address those costs while preserving eager validation and error order. The independent Rust reference already streams over the string and produces the same tested outputs. Separate audited timings put checksum at **1.35× Rust time** for 4,096 digits and **2.96×** for 16 digits, while improving on the preserved Imre baseline by **29.21×** and **16.41×**, respectively. These checksum profiles do not explain the separate `verify`, `generate`, or `append` gaps.

## Compatibility and validation

The [upstream manifest](../../examples/luhn_checksum/UPSTREAM.json) still matches the original module, ten tests, and MIT license byte for byte. Each of **five native configurations** passes all ten upstream tests and five doctest examples through adapters, plus **1,144 CPython differential cases**: ordinary O0/O3, ThinLTO O3, and ASan/LSan O0/O1. Both Rust reference builds pass the same library corpus.

Cases cover all 760 Unicode 16 decimal characters, mixed digit families, empty/random/long inputs, invalid-character ordering, exact exception messages and metadata, malformed UTF-8, and output-file preservation. Separate generic fixtures exercise alias mutation, evaluation order, arbitrary-precision fallbacks, control-flow exits, post-advance reads, and ownership. Luhn has no diagnostic exclusions.

The frozen full matrix passes **129 command jobs**, including formatting, strict workspace Clippy, **190 Rust unit tests**, **4,256 generic checks**, all six application oracles, and unchanged Base45 validation. The 81 language/library suite invocations cover 27 suites across ordinary, ThinLTO, and sanitizer configurations; paired optimization levels are grouped within one invocation. The independent [validation audit](2026-10-20-luhn-performance-validation-audit.json) records exact counts and rehashes every retained native artifact.

GitHub CI also [passes at the measured implementation revision](https://github.com/viarius-experiments/imre/actions/runs/39405086975), using the repository's normal Rust toolchain.

ASan/LSan instruments generated code and the custom runtime with leak detection enabled at O0/O1. The compiler, prebuilt Rust standard library, and Rust references are not sanitizer-instrumented. Historical exhaustive Unicode audits remain applicable by exact byte identity of the relevant implementation; they were not rerun over all code points for this change.

The scope remains the library's string API through a strict UTF-8 driver. It does not add a Python-callable extension module or general escaping iterators.

## Measurement and evidence

Compiler/runtime builds, full validation, timing, and final profiles all use the same clean source revision: **8b54eae6572a93926265d78562656dd395ad28b0**. Later report and documentation commits do not move those validation claims to a new source revision. The [build record](2026-10-20-luhn-performance-build.json) identifies exact source, tools, commands, archives, and executables.

Measurements ran on an AMD EPYC-Milan host pinned to CPU 0, outside ptrace tracing. CPython 3.14.5 uses PGO/LTO, with the GIL enabled and JIT disabled. Imre uses Clang 18 at O3; the independent Rust reference uses Ohm 1.98.1-1, rustc 1.98.1-dev, and LLVM 22.1.8.

Timing includes startup, imports, one input read, repeated unchanged-library calls, checksum accumulation, one final complete output write, and exit. Preparation, compilation, hashing, and verification are outside the clock. OS caches are warm; output is not fsynced. Task-owned builds, tests, profiles, and audit sweeps were paused during measurement. Unrelated shared-host activity and frequency remain uncontrolled.

The records retain **2,184 executions: 1,248 measured, 832 warmups, and 104 correctness gates**, including empty-input controls. The independent [benchmark audit](2026-10-20-luhn-performance-benchmark-audit.json) recomputes summaries and ratios, checks order balance and complete results, and verifies source/build/tool/binary identities.

Exploratory candidates are retained separately from final evidence. Forced LLVM vectorization metadata improved some large-input exploratory timings but produced missed-transformation warnings in supported configurations, so it was removed. The final implementation does not suppress those diagnostics or force that transformation. A subsequent matrix exposed unsupported variadic printing in a new test fixture; the fixture was corrected and the complete final matrix rerun. Both unsuccessful matrices remain in the evidence archive. A runtime inlining experiment did not improve latency and was also excluded. An earlier exploratory build reused an uncommitted runtime through the shared build cache; that mixed-source run is excluded from performance claims. Final builds require fresh compiler invocations from the canonical checkout and bind copied artifacts to those invocations.

Raw records: [ordinary linking](2026-10-20-luhn-performance-normal.json.gz), [ThinLTO](2026-10-20-luhn-performance-thinlto.json.gz), [validation index](2026-10-20-luhn-performance-validation.json), [short profile](2026-10-20-luhn-performance-short-profile.json), and [large profile](2026-10-20-luhn-performance-large-profile.json). The [profile attribution](2026-10-20-luhn-performance-profile-analysis.json) and independent [short](2026-10-20-luhn-performance-short-profile-audit.json)/[large](2026-10-20-luhn-performance-large-profile-audit.json) audits bind interpretation to the raw counters and exact timed binaries.

The [evidence archive](2026-10-20-luhn-performance-evidence.tar.gz) and [member manifest](2026-10-20-luhn-performance-evidence-manifest.json) retain measured sources, emitted LLVM, logs, raw timings and outputs, profiles, audits, and reproduction helpers. Native binaries, runtime archives, interpreters, and toolchains remain external and hash-identified. The previous immutable archive is linked and hashed without nesting it. Original commands retain host-specific paths; the [example guide](../../examples/luhn_checksum/README.md) and [Rust reference guide](../reference/luhn_checksum/README.md) provide portable entrypoints.
