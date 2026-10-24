> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Scalar f-strings: inventory reconciliation and native comparisons

Scalar f-strings now cover the inventory renderer without changing its output or native code. The new and previous programs produce **byte-identical LLVM and native `.text` in both ordinary and ThinLTO builds**. Across twelve comparisons, Imre median times change by −1.31% to +1.34%, with every previous/current interquartile range overlapping. This is a syntax and usability milestone; it makes no native speedup claim.

The implementation is [PR 30](https://github.com/viarius-experiments/imre/pull/30), continuing GitHub Stack 4 after the [inventory string-construction optimization](2026-10-18-inventory-performance.md). Measurements use clean source commit `e9c2a879ff258892ffe8d2c6862b5fa7688eea57`. Only the application's `render.py` changed: explicit concatenations became f-strings, including the newline passed to `write`. The reconciliation algorithm, inputs, output contract, and Rust reference remain unchanged.

## Supported syntax and semantics

Fields accept exact `int`, `bool`, `str`, `None`, and sealed builtin exceptions, with default conversion or `!s` and an absent or explicitly empty format specification. Ordinary, raw, triple-quoted, and adjacent literals, escaped braces, Unicode/NUL data, and nested f-strings inside field expressions are supported. Optional values require existing local narrowing.

```python
line(
    output,
    f"FULFILL {request.batch} {request.row} {request.sku} "
    f"{request.quantity} {result.filled} {result.shortage}",
)
```

Each field expression and conversion finishes before the next field evaluates. Lowering reuses the sealed typed conversions and ordered string builder; earlier strings and integers retain their owners across later mutations or failures. Literal checks preserve each adjacent part's raw-prefix rules, and literal tabs/form feeds remain data. Used and discarded file-write results retain their existing behavior.

Nonempty or dynamic format specifications, `!r`, `!a`, and debug `=` fields are rejected, including debug fields with `!s`. General formatting/repr protocols and builtin-name rebinding remain unsupported. The [language contract](../../docs/subset.md#scalar-f-strings) specifies the boundary.

## Results

Times are median milliseconds; positive Imre change means more time than the previous optimized executable. The [complete summary](2026-10-18-scalar-fstrings-summary.json) retains exact medians, Q1/Q3, all ratios, and overlap checks. Interquartile ranges describe sample spread, not confidence intervals.

**Ordinary**

| Input | CPython ms | Previous Imre ms | Current Imre ms | Rust BigInt ms | Imre time change |
|---|---:|---:|---:|---:|---:|
| empty | 26.526 | 2.158 | 2.159 | 2.214 | +0.04% |
| boundary-128 | 27.534 | 2.760 | 2.768 | 2.589 | +0.28% |
| dense-2048 | 32.774 | 6.484 | 6.399 | 6.003 | -1.31% |
| sparse-2048 | 36.291 | 9.269 | 9.269 | 8.511 | +0.01% |
| dense-16384 | 70.192 | 27.297 | 27.241 | 23.455 | -0.21% |
| sparse-16384 | 101.922 | 51.322 | 51.778 | 42.810 | +0.89% |

**ThinLTO**

| Input | CPython ms | Previous Imre ms | Current Imre ms | Rust BigInt ms | Imre time change |
|---|---:|---:|---:|---:|---:|
| empty | 27.114 | 2.259 | 2.288 | 2.404 | +1.27% |
| boundary-128 | 27.978 | 2.774 | 2.812 | 2.671 | +1.34% |
| dense-2048 | 33.199 | 5.792 | 5.844 | 5.749 | +0.88% |
| sparse-2048 | 36.460 | 8.052 | 8.052 | 7.809 | +0.01% |
| dense-16384 | 69.902 | 21.572 | 21.622 | 20.531 | +0.23% |
| sparse-16384 | 101.639 | 42.465 | 42.639 | 38.046 | +0.41% |

For the two 16,384-positive-request inputs, ThinLTO Imre runs **2.38–3.23× faster than CPython** and takes **1.053–1.121× Rust BigInt's time**: Imre takes 5.3–12.1% more time than Rust in this run. With ordinary linking, Imre runs 1.97–2.58× faster than CPython and takes 1.161–1.209× Rust's time. Small inputs include a substantial startup and small-file floor.

The [native code audit](2026-10-18-scalar-fstrings-native-text.json) compares complete `.text` bytes without modifying or executing either binary. Ordinary `.text` contains 1,032,601 bytes; ThinLTO contains 1,040,928 bytes. Whole executable files differ: the changed sections are `.strtab`, and additionally ThinLTO's build-ID note. No new CPU or allocation profiles were collected.

## Comparison method

Each mode compares current Imre, the immediately preceding optimized Imre executable, CPython 3.14.5, and the same frozen Rust BigInt reference on six captured inputs. CPython and current Imre use the new f-string source; preserved Imre was compiled from the old concatenation source. The explicit baseline-source snapshot option verifies the complete old package against its original record and records the exact change. Historical CPython ratios therefore cannot isolate compiler effects from the source change or host variation.

Each workload has 48 measured rounds covering all 24 engine permutations twice, plus two warmup rounds. Measured positions and directed within-round predecessors are balanced; warmups, cross-round predecessors, and workload order are not. All **2,304 measured executions, 96 warmups, and 48 workload gates** are retained. Every run checks full status, stdout/stderr, report bytes, and filesystem contents against the independent oracle. Before timing, every engine also passes all 176 oracle cases and Linux `/dev/full`: **1,408 oracle outcomes and 8 device outcomes** across both modes.

Every execution is a fresh process pinned to CPU 0, outside ptrace. Timing includes startup/imports, input open/read/decode/parse, eager containers and records, reconciliation, complete report rendering, buffered writes, flush/close, and exit. Fixture creation, oracles, builds, hashing/comparison, and fixture deletion are outside the timer. Inputs are created before timing; OS caches are not deliberately flushed, and writes are not fsynced. CPython's normal bytecode cache is prepared outside timing and verified unchanged before and after both modes.

Ordinary and ThinLTO sessions run sequentially after task-owned builds, tests, and profilers stop. CPU frequency and unrelated shared-host activity are uncontrolled. These are whole-application measurements on one x86-64 AMD EPYC-Milan devbox. CPython is a GIL build with PGO/LTO and its experimental JIT disabled. Imre uses Clang 18.1.3 at `-O3`; ThinLTO uses LLD 22.1.8. Runtime/reference builds use Rust 1.98.1-dev with Ohm experimental defaults disabled; ThinLTO LLVM budgets match at 1,000. Rust preserves the eager algorithm and arbitrary-precision quantities while using native containers, borrowed strings, and direct formatting.

## Validation and evidence

The final compiler/runtime artifacts are byte-identical to the validated development artifacts. All **2,093 semantic-suite checks** pass across ordinary `-O0`/`-O3`, ThinLTO `-O3`, and AddressSanitizer `-O0`/`-O1`; these totals include compile-time rejection checks and are not native process counts. The **65 Rust unit tests**, Clippy/format checks, and 19 pure-Python harness guard checks pass. A separately instrumented Imre application also passes the 176-case inventory oracle alongside CPython and Rust, plus `/dev/full`. Sanitizers cover generated Imre code and runtime dependencies; the prebuilt Rust standard library is not instrumented.

The retained validation includes two superseded f-string fixture failures, in ordinary and sanitizer modes. The initial accepted fixture rebound builtin `str`, which the existing language contract rejects. Correcting the fixture and adding an explicit rebinding rejection resolved both failures without changing compiler behavior. Initial test-file hashes and failure logs are preserved; the two initial uncommitted test-file contents were not captured. The corrected files are in the measured source snapshot. The [validation index](2026-10-18-scalar-fstrings-validation.json) records this reconciliation and its provenance limits.

The [timing audit](2026-10-18-scalar-fstrings-audit.json) verifies **27,962 assertions and 102 distinct file hashes**, including exact output/filesystem checks, sample counts/order, independently recomputed statistics, committed sources, frozen artifacts, and bytecode caches. [Ordinary raw records](2026-10-18-scalar-fstrings-normal.json.gz) and [ThinLTO raw records](2026-10-18-scalar-fstrings-lto.json.gz) preserve the original JSON bytes under deterministic gzip compression. The [build record](2026-10-18-scalar-fstrings-build.json) and [session record](2026-10-18-scalar-fstrings-session.json) retain exact commands, versions, hashes, and source identities.

The [evidence archive](2026-10-18-scalar-fstrings-evidence.tar.gz) contains the measured source snapshot, generated LLVM, previous renderer package, build/validation records and logs, superseded fixture evidence, and exact launcher/audit/packaging scripts. The [manifest](2026-10-18-scalar-fstrings-evidence-manifest.json) hashes every member and companion. Native binaries, runtime archives, interpreters, toolchains, and historical result files are excluded; original paths/hashes and references to the previously published baseline records remain. Audits that read executable/toolchain files need those preserved local artifacts. The published records support statistical recomputation and source/evidence verification independently.
