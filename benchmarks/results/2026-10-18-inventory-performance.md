> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Inventory string construction: CPU, allocations, and Rust comparison

Fusing string construction reduces large inventory times by **29.0–33.6% with ThinLTO** and **32.7–36.0% with ordinary linking**, without changing the program. With ThinLTO, Relar runs **2.52–3.37× faster than CPython** and takes **1.048–1.098× the Rust BigInt reference's time**. Allocation counts fall **68.7–70.9%**; a measurable Rust timing advantage remains.

The application and Rust reference are unchanged. The implementation is [PR 28](https://github.com/viarius-experiments/relar/pull/28), continuing native GitHub Stack 4. Measurements use clean source commit `7ec35cbd0a94ca82419061c725382b2e2a1d4a1e`; its compiler and runtime artifacts are byte-identical to the final development candidate validated before publication.

## What the profiles changed

The original runtime spent most allocation traffic on intermediate concatenations, string literals, and integer-to-string conversion: 79.6% of allocated blocks on dense input and 77.7% on sparse input. Callgrind also showed substantial work in allocation/free paths and repeated `CompactString::push_str` calls.

We now stage each exact string-concatenation tree in source order. Literals borrow immutable UTF-8 spans. Exact builtin `str` conversion leaves format tagged integers into 20-byte function-entry scratch at their original evaluation point; larger integers retain the existing arbitrary-precision conversion. Computed strings remain independently rooted across later mutation and exceptions. After one checked capacity calculation, the runtime copies fragments into their final string storage and publishes the length once.

Discarded file writes omit the unused Unicode character count. When their argument is a direct concatenation, they also construct the complete text without a temporary reference-counted wrapper. The text is still fully assembled before checking file state, and the existing single contiguous write preserves buffering and failure behavior. Writes whose result is used retain their character-count result.

Descriptor arrays and integer buffers have unique entry-block storage, so loops reuse scratch and nested expressions cannot overwrite an earlier fragment. Allocation exhaustion remains terminal. The optimization applies to exact built-in strings and sealed file operations under the existing type and ownership rules.

Expanded sanitizer checks also exposed a pre-existing `-O0` issue: Clang 18 AddressSanitizer moved argument stores ahead of redundant default initialization, which then erased the arguments. Initializing parameter slots exactly once fixes the isolated reproduction and the existing container/file suites. CI now covers those suites at `-O0` as well as the full sanitizer suite at `-O1`.

## CPU and allocation evidence

| Mode | Input | Original instructions | Optimized instructions | Rust instructions | Relar reduction |
|---|---|---:|---:|---:|---:|
| Ordinary | dense-16384 | 346,819,285 | 199,669,704 | 145,098,582 | 42.4% |
| Ordinary | sparse-16384 | 592,952,946 | 360,707,107 | 265,252,293 | 39.2% |
| ThinLTO | dense-16384 | 306,107,298 | 163,147,119 | 123,100,487 | 46.7% |
| ThinLTO | sparse-16384 | 519,573,437 | 292,879,446 | 222,243,071 | 43.6% |

Allocation counts are identical across linking modes. Requested and peak bytes differ by three bytes with executable-path length; this table uses ordinary linking.

| Input | Engine | Allocated blocks | Requested bytes | Peak live requested bytes |
|---|---|---:|---:|---:|
| dense-16384 | Original Relar | 737,634 | 31,104,655 | 2,960,589 |
| dense-16384 | Optimized Relar | 214,685 | 10,470,795 | 2,960,601 |
| dense-16384 | Rust BigInt | 209,498 | 11,313,674 | 4,518,642 |
| sparse-16384 | Original Relar | 1,211,056 | 55,580,015 | 6,718,517 |
| sparse-16384 | Optimized Relar | 378,738 | 22,965,627 | 6,718,449 |
| sparse-16384 | Rust BigInt | 378,629 | 25,877,008 | 9,881,188 |

Requested allocation bytes fall 66.3% on dense input and 58.7% on sparse input. Optimized allocation counts are 2.48% above Rust on dense input and 0.029% above Rust on sparse input; requested bytes are 7.45% and 11.25% below Rust. Peak live requested heap is essentially unchanged by this optimization. The preserved Rust profiles use the exact unchanged reference binaries and captured inputs used in the final timing run; only optimized Relar needed new profile runs.

Callgrind reports executed instructions, not CPU cycles or elapsed-time shares. DHAT reports requested allocation traffic and peak live heap bytes, not process RSS. Hardware performance counters are unavailable on this devbox. Profiled elapsed times are diagnostic only and are excluded from the native timing comparison. Inclusive call costs overlap and are never added together.

The remaining allocation traffic is concentrated in parsing, eager records, owned concatenation results, and complete write buffers. Ordinary-mode stacks retain the runtime entry-point boundaries used for this attribution; ThinLTO inlining moves many allocations into generated functions.

| Allocation entry | Dense blocks | Sparse blocks |
|---|---:|---:|
| `split_whitespace` | 76,145 | 142,445 |
| `record_new` | 54,330 | 90,757 |
| `concat_parts` | 42,514 | 84,270 |
| `text_write_parts` | 21,891 | 24,811 |
| `splitlines` | 19,080 | 35,663 |

The final traces contain no allocating `int_str` entry and no executed `CompactString::push_str` function. The six remaining ordinary-mode literal allocations in each workload are negligible. ThinLTO self-instruction counts include 15.65M/25.00M in the dense/sparse renderer, 14.01M/17.70M in dictionary insertion, and 7.37M/15.69M in token splitting. These exclude callees. Total ThinLTO instructions remain 32.5%/31.8% above Rust, while native elapsed times are 4.8%/9.8% higher; instruction counts and elapsed-time ratios are not interchangeable.

The unchanged renderer still materializes a string before its `line` helper adds the newline. The Rust reference formats directly into its writer. Further work could investigate that call boundary and record/container ownership, guided by these profiles.

## Timing method

Each mode compares the preserved original inventory executable, optimized Relar, CPython, and the unchanged Rust BigInt reference on six inputs. It also compares CPython, preserved Relar, and optimized Relar on the five existing incremental-planner controls. Every inventory workload has 48 measured rounds containing all 24 engine permutations twice; controls use all six permutations eight times. There are two warmups per engine. The measured rounds balance engine positions and directed within-round predecessors; warmups, cross-round predecessors, and workload order are not balanced.

All 3,744 measured executions and 156 warmups are retained. Each uses a fresh process pinned to CPU 0, outside ptrace. Timing includes startup, input open/read/decode/parse, eager collection construction, reconciliation, complete report rendering/writes, flush/close, and exit. Fixture preparation, oracle execution, hashing/comparison, builds, and deletion are outside the timer. Inputs are created before timing and executables are exercised by validation/warmups; OS page caches are not deliberately flushed, and output is not fsynced. CPython uses its normal bytecode cache, whose application/control cache bytes are recorded before and after both sessions.

Ordinary and ThinLTO runs are sequential, after all task-owned builds, tests, and profilers stop. CPU frequency and other shared-machine activity are uncontrolled. These are complete-application measurements on one shared devbox. The Rust implementation follows the same eager algorithm and arbitrary-precision contract, using native containers, borrowed strings, and direct formatting. The comparison includes those representation differences. No C implementation was measured.

## Inventory results

Times are median milliseconds with observed Q1–Q3 in brackets. These ranges describe sample spread, not confidence intervals. Ratios divide medians; `Relar/Rust` above one means Rust is faster. The preserved and optimized executables run in the same session.

**Ordinary**

| Workload | CPython ms | Preserved Relar ms | Optimized Relar ms | Rust BigInt ms | Before/Relar | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| empty | 26.911 [26.754–27.102] | 2.250 [2.175–2.385] | 2.221 [2.114–2.280] | 2.266 [2.207–2.408] | 1.013× | 12.115× | 0.980× |
| boundary-128 | 28.182 [27.775–29.174] | 3.087 [2.989–3.229] | 2.863 [2.754–2.966] | 2.754 [2.634–2.840] | 1.078× | 9.845× | 1.039× |
| dense-2048 | 33.919 [33.470–34.536] | 8.275 [8.059–8.557] | 6.596 [6.467–6.766] | 6.077 [5.970–6.329] | 1.255× | 5.142× | 1.085× |
| sparse-2048 | 38.067 [37.273–40.571] | 13.018 [12.473–13.546] | 9.578 [9.277–9.891] | 8.812 [8.597–9.024] | 1.359× | 3.974× | 1.087× |
| dense-16384 | 74.821 [73.398–76.720] | 43.193 [41.099–44.822] | 27.644 [27.074–28.113] | 23.832 [23.380–24.498] | 1.562× | 2.707× | 1.160× |
| sparse-16384 | 107.831 [105.856–110.407] | 77.122 [74.215–78.396] | 51.918 [51.162–52.939] | 43.736 [43.074–44.767] | 1.485× | 2.077× | 1.187× |

**ThinLTO**

| Workload | CPython ms | Preserved Relar ms | Optimized Relar ms | Rust BigInt ms | Before/Relar | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| empty | 26.924 [26.601–27.554] | 2.261 [2.123–2.451] | 2.211 [2.083–2.383] | 2.336 [2.227–2.518] | 1.023× | 12.180× | 0.946× |
| boundary-128 | 27.868 [27.550–28.212] | 2.928 [2.844–3.091] | 2.804 [2.681–2.915] | 2.722 [2.624–2.868] | 1.044× | 9.937× | 1.030× |
| dense-2048 | 32.664 [32.545–32.862] | 7.056 [6.992–7.161] | 5.633 [5.548–5.762] | 5.606 [5.487–5.721] | 1.253× | 5.799× | 1.005× |
| sparse-2048 | 36.232 [35.951–36.538] | 10.084 [9.998–10.183] | 7.840 [7.749–7.926] | 7.695 [7.607–7.850] | 1.286× | 4.622× | 1.019× |
| dense-16384 | 73.016 [72.470–74.093] | 32.671 [32.401–33.483] | 21.697 [21.356–22.079] | 20.710 [20.385–21.101] | 1.506× | 3.365× | 1.048× |
| sparse-16384 | 106.178 [105.258–107.378] | 59.335 [58.669–60.070] | 42.125 [41.485–42.836] | 38.354 [37.651–38.970] | 1.409× | 2.521× | 1.098× |

The two largest inputs each contain 16,384 positive requests and 18,906 total requests. Dense input repeats 64 stock SKUs; sparse input uses many unique and unknown SKUs. Their complete reports contain 654,942 and 1,438,871 bytes. The boundary input retains Unicode names, duplicates, unknown requests, and quantities around `10**80`. Empty and small inputs include a substantial process-startup and small-file floor; their CPython ratios do not establish equivalent computational speedups.

## Existing planner controls

| Mode | Workload | CPython ms | Preserved Relar ms | Optimized Relar ms | Time change |
|---|---|---:|---:|---:|---:|
| Ordinary | layered-16384-sparse | 111.992 [110.922–113.314] | 49.433 [48.676–49.846] | 49.674 [48.640–49.976] | +0.49% |
| Ordinary | layered-16384-all | 194.570 [192.794–198.812] | 75.670 [74.703–77.453] | 75.866 [74.501–77.268] | +0.26% |
| Ordinary | wide-16384-sparse | 104.174 [103.424–105.392] | 26.766 [26.611–27.163] | 27.005 [26.799–27.252] | +0.89% |
| Ordinary | wide-16384-all | 188.805 [188.111–190.737] | 45.262 [44.800–45.758] | 45.530 [45.256–45.923] | +0.59% |
| Ordinary | boundary-128-all | 21.043 [20.703–21.572] | 3.678 [3.584–3.791] | 3.601 [3.534–3.721] | -2.09% |
| ThinLTO | layered-16384-sparse | 113.907 [113.066–115.598] | 43.136 [42.649–43.822] | 43.088 [42.549–43.759] | -0.11% |
| ThinLTO | layered-16384-all | 191.732 [189.960–194.091] | 63.988 [63.265–65.319] | 64.083 [63.151–65.089] | +0.15% |
| ThinLTO | wide-16384-sparse | 102.487 [101.726–103.491] | 21.430 [21.317–21.746] | 21.481 [21.282–21.681] | +0.24% |
| ThinLTO | wide-16384-all | 188.410 [186.854–190.562] | 36.806 [36.251–38.305] | 37.188 [36.345–38.516] | +1.04% |
| ThinLTO | boundary-128-all | 20.816 [20.602–21.689] | 3.478 [3.417–3.562] | 3.467 [3.420–3.563] | -0.31% |

All ten optimized-control IQRs overlap their preserved-baseline IQRs. Median changes range from −2.09% to +1.04%; these runs show no clear control regression. Their Python sources are unchanged, but generated LLVM differs because the compiler changed. We do not claim LLVM identity or infer exact equivalence of performance from overlapping IQRs.

## Semantics and validation

The independent inventory oracle checks all 176 filesystem cases (127 successful and 49 expected terminal failures), plus Linux `/dev/full`, for every timing engine before measurement. It checks complete report bytes, status/error category, and filesystem contents. Workload gates, warmups, and every measured execution receive complete checks too. The unchanged incremental planner retains its 172-case oracle.

Input ordering, last-wins stock with first insertion order, ignored nonpositive requests, unknown SKUs, arbitrary-precision quantities, Unicode/NUL handling, input/output aliases and hard links, parse-before-output-open behavior, and terminal output failures remain unchanged. The [application contract](../../examples/inventory_reconciliation/README.md) defines the exact shared domain.

All eight semantic suites pass: **710 checks at ordinary `-O0`/`-O3`, 533 at ThinLTO `-O3`, and 710 with AddressSanitizer at `-O0`/`-O1`**, plus 10 ThinLTO CLI checks. These suite totals include compile-time rejections and are not all native executions. A separately instrumented inventory executable passes all 176 oracle cases and `/dev/full`. The 65 Rust unit tests, Clippy, formatting, and Ruff checks pass. There are also 45 pure-Python harness/preflight checks; they are not native executions.

The retained timing audit verifies 39,569 assertions across 3,744 measured runs, 156 warmups, 48 workload gates, and 2,448 oracle/parser/device outcomes. The semantic index checks 177 file hashes and establishes exact source/artifact identity for reused development-candidate validation. Unit/Clippy commands and zero exits are explicitly recorded as root-observed results, with their logs. The profile audit reparses all 24 baseline/final/Rust traces and verifies 222 sealed profile files.

New regressions exercise both concat tree associations, expression/conversion order, field/list replacement, aliases, integer tag boundaries and large values, Unicode/NUL/empty fragments, the 23/24/25-byte inline/heap boundary, loop scratch reuse, early/late failures, closed/read-only empty writes, receiver lifetime, large writes, and `/dev/full` cleanup. Sanitizers instrument generated code and the Rust runtime/target dependencies with leak detection outside ptrace; the prebuilt Rust standard library is not instrumented.

The host is an x86-64 AMD EPYC-Milan Linux VM. Native compilation uses Clang 18.1.3 at `-O3`; ThinLTO uses LLD 22.1.8. Runtime/reference builds use Rust 1.98.1-dev (`ohm-1.98.1-1`, LLVM 22.1.8) with Ohm's experimental defaults disabled. CPython 3.14.5 is a GIL build with PGO and LTO enabled and its experimental JIT disabled. Exact flags, hashes, versions, commands, environment allowlists, and build logs are retained.

## Next compatibility milestone

The next bounded capability should be scalar f-strings. Ruff already parses their structure, and the new ordered string construction path provides the backend. Start with ordinary/raw/triple-quoted and adjacent f-strings, escaped braces, and fields whose exact types already support `str`: integers, booleans, strings, `None`, and sealed exceptions. Support default conversion or `!s` with an empty format specification; require existing narrowing for optional values.

Each field expression and its conversion must complete before the next field evaluates. Lower directly to the sealed typed conversion operation so a local name `str` cannot change f-string behavior. Nonempty/dynamic format specifications, `!r`, `!a`, and debug `=` need explicit formatting/repr contracts and should initially receive clear compile errors. Literal surrogate-escape validation must account for each adjacent part's raw prefix and recursively inspect field expressions. Ruff's flattened element iterator omits adjacent ordinary literals, so lowering must preserve the complete part sequence. Multiline literal tabs/form feeds must also remain data rather than trigger code-indentation checks.

The acceptance example is rewriting this same inventory renderer with expressions such as `f"FULFILL {request.batch} {request.row} {request.sku} {request.quantity} {result.filled} {result.shortage}"`, retaining the complete oracle and performance checks. This is a syntax/usability milestone; equivalent fused concatenation should already have the same optimized execution path. Acceptance cases should also cover mixed raw/ordinary adjacency, escaped braces/NUL, nested f-strings in field expressions, explicit empty `:`, `KeyError` quoting, narrowed optionals, a shadowed `str` name, ownership across later mutation/failure, and used/discarded write results. Debug `=` must be rejected even with `!s`, as must dynamic specifications that happen to produce an empty string. Keyword/default arguments can follow with a separate argument-binding and definition-time default-storage design.

## Evidence and reproduction

The [ordinary samples](2026-10-18-inventory-performance-normal.json.gz) and [ThinLTO samples](2026-10-18-inventory-performance-lto.json.gz) preserve the raw JSON bytes under deterministic gzip compression. The [timing audit](2026-10-18-inventory-performance-audit.json), [profile audit](2026-10-18-inventory-performance-profiles.json), [semantic validation index](2026-10-18-inventory-performance-validation.json), and [frozen build record](2026-10-18-inventory-performance-build.json) record the checks and identities behind the tables.

The [evidence archive](2026-10-18-inventory-performance-evidence.tar.gz) contains the measured source snapshot, generated LLVM, baseline/final raw traces, complete retained profile outputs, validation logs and runner records, timing/profiling launchers, audit scripts, and the isolated sanitizer diagnosis. The [manifest](2026-10-18-inventory-performance-evidence-manifest.json) hashes every archive member and companion file. Native binaries, runtime archives, interpreters, and toolchains are excluded; their paths and hashes remain in the records. Audits that need those artifacts require the preserved local files. Raw timing and trace data support statistical recomputation and trace parsing independently.

Use the [benchmark guide](../README.md) for builds and comparisons. `inventory_benchmark.py` accepts a preserved inventory executable and its record through `--inventory-baseline` and `--inventory-baseline-record`; `inventory_profile.py` profiles exact artifacts from a completed timing record without rebuilding them. The archived launchers preserve the exact commands used here. Rebuilds should produce new records rather than replace these retained observations.
