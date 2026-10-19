> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Reducing the native Base45 decoding gap

With python-base45 unchanged, decoding is **10.43–15.10× faster than the previous Relar executable** in a same-session comparison. Sustained decoding now takes **3.57–4.61× Rust's time**; the 16 KiB bytes case takes **3.62×**. This substantially narrows the gap, but does not establish Rust parity. ThinLTO decoding is **19.16–24.64× faster than CPython** in the shorter three-engine comparisons.

The [compiler/runtime change](https://github.com/viarius-experiments/relar/pull/51) follows PR #50 in GitHub Stack 4. The current compiler and runtime were built from clean commit **7c1e378625d6760b30e68025fe2299b29287cedc**. Application, harness, and Rust reference source bytes match that revision; the unchanged Rust binaries were retained from their recorded earlier builds. The [previous encoder report](2026-10-19-base45-string-reuse.md) and its evidence remain intact.

## Sustained comparison with Rust

Each process handles **64 MiB of raw data** through repeated calls on the indicated payload. Values are median milliseconds with inclusive quartiles in brackets. **Relar / Rust is a time ratio: lower is better.** Both native executables use ThinLTO, with matched LLVM import and inline budgets of 1,000. Twenty balanced AB/BA measured rounds follow two balanced warmup rounds.

| Operation / raw bytes per call | Relar ms [Q1–Q3] | Rust ms [Q1–Q3] | Relar / Rust |
|---|---:|---:|---:|
| encode-64 | 466.563 [465.258–471.927] | 192.020 [191.107–193.685] | 2.430× |
| decode-bytes-64 | 411.743 [409.244–412.809] | 97.856 [97.549–98.370] | 4.208× |
| decode-text-64 | 430.204 [425.210–432.181] | 93.357 [92.891–93.919] | 4.608× |
| encode-1024 | 295.584 [294.271–296.059] | 176.807 [175.660–177.784] | 1.672× |
| decode-bytes-1024 | 297.044 [295.911–298.732] | 83.193 [82.630–83.579] | 3.571× |
| decode-text-1024 | 293.852 [292.731–296.720] | 77.979 [77.530–78.513] | 3.768× |
| encode-16384 | 270.887 [269.426–272.093] | 175.492 [174.235–176.476] | 1.544× |
| decode-bytes-16384 | 294.715 [293.259–295.969] | 81.374 [80.741–81.764] | 3.622× |
| decode-text-16384 | 290.772 [289.902–293.287] | 76.918 [76.552–77.829] | 3.780× |

Encoding is included as a regression control. These longer native batches reduce startup's share of each sample and are the primary assessment of the remaining Rust gap. The [Rust reference](../reference/base45_codec/README.md) is unchanged: a fixed lookup table, preallocated output, direct arithmetic, complete output and checksum checks, and black-box barriers around repeated work. Both implementations handle identical inputs and repetition counts. Their storage strategies differ: the unchanged Python decoder materializes integer lists, while Rust streams lookup and output production and defers range errors to preserve lookup-error priority.

## Same-session comparison with previous Relar

Retained previous and current ThinLTO executables were interleaved on CPU 0 with identical inputs and **2 MiB of raw data per process**. Twenty balanced AB/BA measured rounds follow two warmup rounds. The codec and driver hashes match; every complete result and checksum is checked. Only the native executable changes within this control.

| Operation / raw bytes per call | Previous Relar ms [Q1–Q3] | Current Relar ms [Q1–Q3] | Previous / current |
|---|---:|---:|---:|
| encode-64 | 17.850 [17.755–18.002] | 17.959 [17.914–18.105] | 0.994× |
| decode-bytes-64 | 178.927 [177.854–180.311] | 16.499 [16.309–16.697] | 10.845× |
| decode-text-64 | 177.625 [176.314–179.348] | 17.037 [16.830–17.542] | 10.426× |
| encode-1024 | 12.930 [12.828–13.213] | 12.971 [12.802–13.247] | 0.997× |
| decode-bytes-1024 | 166.205 [165.638–168.171] | 13.043 [12.811–13.435] | 12.743× |
| decode-text-1024 | 165.354 [164.674–166.200] | 12.891 [12.722–13.156] | 12.828× |
| encode-16384 | 12.024 [11.893–12.074] | 12.021 [11.953–12.121] | 1.000× |
| decode-bytes-16384 | 199.158 [197.564–200.474] | 13.188 [13.026–13.579] | 15.102× |
| decode-text-16384 | 165.576 [164.154–168.062] | 12.946 [12.838–13.302] | 12.790× |

This is the direct before/after measurement. Comparing separately collected historical medians would mix the compiler change with host conditions. The new Rust comparisons independently establish the remaining native gap.

## Ordinary linking: Relar, Rust, and CPython

Each process handles **2 MiB of raw data**. Six balanced warmup rounds precede 24 measured rounds covering all six engine orders four times. **CPython / Relar is a speedup ratio: higher is better.**

| Operation / raw bytes per call | Relar ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Relar / Rust | CPython / Relar |
|---|---:|---:|---:|---:|---:|
| encode-64 | 22.950 [22.715–23.150] | 9.224 [9.135–9.299] | 327.714 [326.297–330.426] | 2.488× | 14.279× |
| decode-bytes-64 | 29.372 [28.962–29.949] | 7.509 [7.172–7.828] | 329.292 [325.502–335.081] | 3.912× | 11.211× |
| decode-text-64 | 29.025 [28.638–29.381] | 6.748 [6.564–6.974] | 323.828 [321.396–327.642] | 4.301× | 11.157× |
| encode-1024 | 17.031 [16.825–17.360] | 9.253 [9.012–9.380] | 339.717 [336.238–342.292] | 1.841× | 19.947× |
| decode-bytes-1024 | 22.870 [22.452–23.428] | 6.769 [6.473–7.333] | 316.445 [313.158–321.129] | 3.379× | 13.837× |
| decode-text-1024 | 22.184 [22.091–22.556] | 6.274 [6.100–6.413] | 317.520 [311.615–321.323] | 3.536× | 14.313× |
| encode-16384 | 16.030 [15.886–16.311] | 9.035 [8.819–9.391] | 341.488 [339.632–345.171] | 1.774× | 21.303× |
| decode-bytes-16384 | 22.507 [22.243–22.675] | 6.399 [6.174–6.623] | 318.052 [313.256–320.557] | 3.517× | 14.131× |
| decode-text-16384 | 22.756 [22.531–22.995] | 6.578 [6.304–6.804] | 314.891 [310.496–317.713] | 3.459× | 13.838× |

## ThinLTO: Relar, Rust, and CPython

| Operation / raw bytes per call | Relar ms [Q1–Q3] | Rust ms [Q1–Q3] | CPython ms [Q1–Q3] | Relar / Rust | CPython / Relar |
|---|---:|---:|---:|---:|---:|
| encode-64 | 18.705 [18.122–19.143] | 9.527 [9.371–9.798] | 333.151 [327.682–339.172] | 1.963× | 17.811× |
| decode-bytes-64 | 16.607 [16.076–16.900] | 6.684 [6.434–7.079] | 326.328 [323.243–332.766] | 2.485× | 19.651× |
| decode-text-64 | 16.837 [16.676–17.210] | 6.527 [6.331–6.717] | 322.659 [321.010–328.190] | 2.580× | 19.164× |
| encode-1024 | 13.094 [12.920–13.528] | 9.043 [8.863–9.432] | 342.692 [337.578–349.865] | 1.448× | 26.172× |
| decode-bytes-1024 | 13.283 [13.065–13.480] | 6.121 [5.875–6.633] | 318.239 [316.121–320.235] | 2.170× | 23.959× |
| decode-text-1024 | 12.961 [12.796–13.166] | 5.995 [5.796–6.169] | 319.315 [316.084–320.808] | 2.162× | 24.636× |
| encode-16384 | 12.314 [12.112–12.511] | 9.037 [8.968–9.312] | 341.097 [339.829–342.757] | 1.363× | 27.701× |
| decode-bytes-16384 | 13.659 [13.355–14.166] | 6.407 [6.124–6.705] | 321.097 [319.357–327.954] | 2.132× | 23.508× |
| decode-text-16384 | 13.610 [13.229–13.813] | 6.366 [6.114–6.797] | 322.254 [316.740–325.839] | 2.138× | 23.677× |

Ordinary and ThinLTO families ran separately, each with interleaved baselines. They compare engines within each family; they are not a randomized measurement of ThinLTO's isolated effect. Quartiles describe observed spread, not confidence intervals.

## Bottlenecks and changes

The previous CPU and allocation profiles showed repeated character-object allocation, dictionary hashing, dynamic integer work, and temporary-list storage dominating decoding. The optimizations are general compiler/runtime transformations; there is no Base45 intrinsic or source rewrite.

- Exact string-to-integer-dictionary comprehensions become bulk gathers. A lazy ASCII cache stays synchronized with dictionary insertion, replacement, and deletion. Unicode keys and arbitrary-precision values retain the general path. Direct bytes decoding validates the entire UTF-8 input at the original evaluation point, then gathers without allocating an intermediate string.
- Eligible leaf range loops get a guarded native-integer version. The compiler checks actual list contents and excludes input/output aliases at runtime. It preserves the original checked loop as fallback, including BigInt values, exceptions, loop targets, break, continue, and else. Proven power-of-two division and modulo become shifts and masks.
- Known-size list producers reserve bounded capacity, and a thread-local pool reuses at most two empty integer-list buffers, retaining at most **512 KiB per thread**. Immediate-integer cleanup and conversion to bytes use vectorizable checks. Adjacent appends share a capacity check only when both values and the common list receiver are already evaluated locals.

The guards establish the facts used by LLVM. They do not assume type annotations or apparently constant dictionaries prevent mutation. Fast paths preserve error ordering: malformed UTF-8 still wins before character lookup, and missing characters are detected before numeric range failures.

## Allocation and CPU-work profiles

Fresh, separate DHAT and Callgrind runs compare the previous executable, current executable, and unchanged Rust reference. Each process handles **64 KiB of raw data**: 1,024 calls at 64 bytes, 64 calls at 1 KiB, or four calls at 16 KiB. Every profile checks the complete output and checksum and records exact input, source, binary, tool, and command identities.

| Operation / raw bytes per call / engine | Allocated bytes | Allocation blocks | Peak live bytes | Instructions |
|---|---:|---:|---:|---:|
| decode-bytes-64 / previous Relar | 7,430,350 | 116,821 | 14,434 | 74,633,000 |
| decode-bytes-64 / current Relar | 215,539 | 4,185 | 16,775 | 6,328,141 |
| decode-bytes-64 / Rust | 69,758 | 1,042 | 2,158 | 1,883,230 |
| decode-bytes-1024 / previous Relar | 7,278,194 | 100,053 | 33,198 | 70,908,970 |
| decode-bytes-1024 / current Relar | 120,983 | 345 | 38,379 | 4,846,856 |
| decode-bytes-1024 / Rust | 71,200 | 82 | 5,520 | 1,626,948 |
| decode-bytes-16384 / previous Relar | 7,291,636 | 98,525 | 455,600 | 70,614,162 |
| decode-bytes-16384 / current Relar | 444,025 | 105 | 391,125 | 4,768,792 |
| decode-bytes-16384 / Rust | 94,241 | 22 | 59,281 | 1,611,555 |
| decode-text-64 / previous Relar | 7,332,220 | 115,800 | 14,432 | 74,731,019 |
| decode-text-64 / current Relar | 256,673 | 5,212 | 16,773 | 6,566,864 |
| decode-text-64 / Rust | 69,755 | 1,042 | 2,155 | 1,818,746 |
| decode-text-1024 / previous Relar | 7,181,504 | 99,992 | 34,772 | 70,658,236 |
| decode-text-1024 / current Relar | 125,157 | 412 | 38,377 | 4,823,991 |
| decode-text-1024 / Rust | 71,197 | 82 | 5,517 | 1,577,530 |
| decode-text-16384 / previous Relar | 7,217,986 | 98,524 | 480,214 | 70,823,342 |
| decode-text-16384 / current Relar | 468,839 | 112 | 415,739 | 4,763,551 |
| decode-text-16384 / Rust | 94,238 | 22 | 59,278 | 1,573,117 |

For 16 KiB bytes decoding, cumulative allocated bytes fall from **7,291,636 to 444,025** (16.4× lower), allocation blocks fall from **98,525 to 105**, and instrumented instructions fall from **70,614,162 to 4,768,792** (14.8× lower). Peak live allocation changes from **455,600 to 391,125 bytes** (-14.2%). Cumulative allocation is not peak memory or RSS; bounded buffer retention trades some idle memory for fewer allocations across calls.

The remaining gap is principally the work still expressed by the Python algorithm: gather an integer list, prove its element range, build another integer list, validate/copy it into bytes, and manage those objects. Rust produces output directly. The named decoder accounts for **63.9% of current process instructions** in the large-bytes profile; current Relar still executes **2.96× Rust's instructions** and requests **4.71× its cumulative allocation**. The next large opportunity appears to be eliminating intermediate containers or fusing their consumers while preserving eager evaluation and error priority.

The [reviewed assembly breakdown](2026-10-19-base45-decoder-profile-hotspots.json) identifies the largest remaining regions in the 16 KiB bytes case:

| Remaining work | Process instructions | Attribution |
|---|---:|---|
| native decode loop: checked source reads, arithmetic, paired append, cleanup | 45.35% | Exclusive decoder region |
| Gather mapped integers | 19.05% | Inclusive helper |
| list[int] to bytes conversion and result allocation | 9.75% | Exclusive decoder region |
| vectorized i32 list guard | 8.76% | Exclusive decoder region |
| Release both integer lists | 6.04% | Inclusive helper calls |

The checked source loads and three-read branch account for **17.18% of process instructions**, as a subset of the native loop above. Decoder regions are disjoint self-cost ranges; helper rows include their callees. These are instruction shares, not CPU-time percentages.

Hardware perf events were unavailable on this host. Callgrind reports instrumented guest instructions, not hardware cycles or CPU-time percentages. Profiles include startup, imports, file I/O, and checksum work and ran separately from wall-clock timing. Decoder attribution resolves the named Python function through its LLVM body and native symbols, then checks every recorded instruction address against retained disassembly.

## Semantics and validation

The codec, py.typed, upstream tests, and BSD-2-Clause license match all four hashes in the [upstream manifest](../../examples/base45_codec/UPSTREAM.json), pinned to [python-base45 commit 5f49a519](https://github.com/kirei/python-base45/tree/5f49a519c3884a4fbc4c58c5c8f1fa25d7046020). The separate upstream argparse application is outside this example.

At each of five configurations—ordinary O0/O3, ThinLTO O3, and AddressSanitizer/LeakSanitizer O0/O1—the unchanged upstream methods cover **323 native calls**, followed by **289 CPython differential cases** and **two compile-time type rejections**. Those last two reject upstream calls passing integer 42 to typed APIs; they are not runtime TypeError tests. Checks cover full values, exact error messages, causes, suppression, unchanged output on failure, malformed/odd/empty inputs, Unicode, all bytes, alias mutation, and recovery.

All **143 Rust unit tests**, strict workspace Clippy, formatting, and focused Ruff checks pass. The full matrix passes **81 jobs**, including **3,931 non-Base45 language checks** across the five configurations. All six existing application oracle suites pass at ordinary O3. Each retained Rust Base45 reference passes **295 checks**. Four recorded Unicode repr boundaries permit different character quoting in Rust while still checking error category, cause, suppression and unchanged output; Relar retains exact CPython message checking. Sanitizers cover generated code and the custom runtime; the prebuilt Rust standard library is uninstrumented.

The first matrix attempt was interrupted after its temporary runner misparsed the successful imports-suite summary; that child exited zero. The second attempt passed all ordinary, ThinLTO, reference and application jobs, but LeakSanitizer could not inspect processes under the sandbox's ptrace tracing. The complete successful matrix reran outside tracing with leak detection enabled and the same frozen binaries. Both earlier attempts are retained and excluded from final counts. The [validation index](2026-10-19-base45-decoder-validation.json), [summary](2026-10-19-base45-decoder-validation-summary.json), and [build/check record](2026-10-19-base45-decoder-build-validation.json) retain exact commands, statuses and provenance.

## Measurement and evidence

Timing ran on October 19, 2026, on an AMD EPYC-Milan host pinned to CPU 0, outside tracing. CPython 3.14.5 has PGO and LTO enabled, the GIL enabled, and JIT disabled. Relar uses Clang 18 at O3; the Rust reference uses Ohm 1.98.1 and LLVM 22.1.8. ThinLTO uses LLVM 22 LLD and linker-plugin runtime bitcode. The [frozen build record](2026-10-19-base45-decoder-build.json) identifies exact tools, source, commands, archives and executables. Source was committed before the recorded builds.

Timing includes startup, imports, one input read, all repeated codec calls, checksum work, one complete final output write, and exit. Compilation, preparation, hashing and verification are outside the clock. OS caches are warm, output is not fsynced, and task-owned builds, tests and profiles were stopped during timing. Host frequency and unrelated shared-host activity remain uncontrolled. Timed inputs have valid even lengths; separate correctness checks cover other cases.

Across the four records, **2,016 measured executions, 396 warmups, and 90 gates** retain exit status, stdout/checksum, stderr, and complete output hashes. The [independent benchmark audit](2026-10-19-base45-decoder-audit.json) recomputes all statistics and verifies balanced orders, oracle results, and source/tool/binary identities. The [profile audit](2026-10-19-base45-decoder-profile-audit.json) independently checks all **48 profiles**, raw allocation/instruction totals, full outputs, manifests and decoder attribution.

Raw records: [ordinary linking](2026-10-19-base45-decoder-normal.json.gz), [ThinLTO](2026-10-19-base45-decoder-thinlto.json.gz), [sustained native comparisons](2026-10-19-base45-decoder-sustained.json.gz), and [previous/current control](2026-10-19-base45-decoder-before-after.json.gz). The [profile summary](2026-10-19-base45-decoder-profile.json) and [comparison](2026-10-19-base45-decoder-profile-comparison.json) retain detailed results.

The [evidence archive](2026-10-19-base45-decoder-evidence.tar.gz) and [manifest](2026-10-19-base45-decoder-evidence-manifest.json) contain measured source and fixtures, generated LLVM, exact build/check logs, raw timing and correctness records, previous/current profiles, disassembly, independent audits, and reproduction scripts. Native binaries, runtime archives, interpreters and toolchains remain external and hash-identified. Original commands retain host-specific paths; the [example guide](../../examples/base45_codec/README.md) and [Rust reference guide](../reference/base45_codec/README.md) provide portable entrypoints.
