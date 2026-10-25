> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Named-local tuple keys in the uv catalog

The unchanged uv catalog builds `labels = (artifact.platform, artifact.variant)` for each artifact. The compiler captures those two strings once, probes the dictionary with a key on the stack, and creates an owned tuple on the cache-insertion path. The hand rewrite captures the strings in two Python locals and uses inline tuple literals for the probe and insertion. It leaves the outer version cache unchanged.

## Merge check on main `98671de`

After the runtime started reusing short fields from split strings, we rebuilt all three executables and repeated the end-to-end comparison. Times are whole-process medians in milliseconds. Each speedup is a paired median with a 95% interval.

| uv plain index | Main ms | Hand ms | Compiler ms | Main / hand | Main / compiler | Hand / compiler |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1×, 48 blocks | 116.86 | 114.75 | 115.20 | 1.0157× [1.0075, 1.0235] | 1.0131× [1.0041, 1.0225] | 0.9976× [0.9942, 1.0069] |
| 10×, 48 blocks | 770.53 | 751.99 | 756.48 | 1.0232× [1.0216, 1.0287] | 1.0183× [1.0092, 1.0228] | 0.9957× [0.9889, 0.9992] |

The independent 10× run used 60 pairs on another core and fresh builds. Main took 772.90 ms, the candidate took 756.80 ms, and the paired speedup was 1.0237× [1.0181, 1.0273].

| Ruff diff | Main ms | Compiler ms | Main / compiler |
| --- | ---: | ---: | ---: |
| 1×, 48 blocks | 103.15 | 102.94 | 1.0011× [0.9990, 1.0033] |
| 10×, 48 blocks | 996.50 | 998.61 | 1.0002× [0.9965, 1.0017] |

All three Ruff arms produced the same executable and LLVM on this runtime. Across uv and Ruff, each operation count in the table below matched the previous runtime. The 66-case catalog and 13-case Ruff suites and mutation comparisons also passed on the merge runtime.

## Full study on main `0412f8b`

Wall times are medians in milliseconds. Speedups are medians of paired elapsed-time ratios, with 95% bootstrap intervals. A ratio above 1 favors the denominator. The two summaries can differ because each ratio uses runs from the same randomized block.

| uv plain index | Main ms | Hand ms | Compiler ms | Main / hand | Main / compiler | Hand / compiler |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1×, 72 blocks | 115.99 | 114.14 | 114.24 | 1.0146× [1.0092, 1.0201] | 1.0153× [1.0065, 1.0236] | 1.0022× [0.9947, 1.0108] |
| 10×, 96 blocks | 767.69 | 751.08 | 752.02 | 1.0213× [1.0152, 1.0262] | 1.0202× [1.0164, 1.0228] | 0.9997× [0.9937, 1.0037] |

The independent 10× confirmation rebuilt both executables, verified their bytes, and used a different CPU and seed for 72 pairs. Main took 762.62 ms and the compiler took 750.19 ms. The paired speedup was 1.0146× [1.0103, 1.0182].

| Ruff diff | Main ms | Compiler ms | Main / compiler |
| --- | ---: | ---: | ---: |
| 1×, 48 blocks | 104.86 | 105.73 | 0.9972× [0.9875, 1.0049] |
| 10×, 144 blocks | 1005.72 | 1010.20 | 0.9955× [0.9911, 0.9985] |
| 1×, 192 independent blocks | 103.27 | 103.48 | 0.9979× [0.9954, 1.0004] |

Ruff already uses an inline tuple key. All three arms produced the same complete reporter LLVM and ELF. Each control includes the real Python reporter’s filesystem traversal, two analyzer replay subprocesses and diff output. The timing intervals quantify process variation on identical code.

## Dynamic operations

| uv index | Arm | Owned labels tuples | Fresh heap tuples | Reused | Stack probes | Retain/release ABI calls |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1× | Main | 58,721 | 795 | 57,926 | 0 | 2,475,758 |
| 1× | Hand | 229 | 229 | 0 | 58,721 | 2,654,198 |
| 1× | Compiler | 229 | 229 | 0 | 58,721 | 2,419,314 |
| 10× | Main | 587,210 | 5,898 | 581,312 | 0 | 19,101,755 |
| 10× | Hand | 229 | 229 | 0 | 587,210 | 20,875,868 |
| 10× | Compiler | 229 | 229 | 0 | 587,210 | 18,527,028 |

At 10×, 538,661 of 587,210 label lookups hit. The cache inserted 229 distinct keys. Main already reuses most owned tuple buffers. The compiler saved 5,669 fresh allocations and 574,727 emitted refcount calls relative to main. Relative to the hand rewrite it eliminated 1,174,420 string retains and 1,174,420 releases by borrowing the saved values during an immediate probe.

The counters ran in separate binaries. They attribute tuple operations to the label descriptor in the selection function, and count retain/release calls from all generated code into the runtime. A cached tuple used a fresh allocation when the returned pointer differed from the old one. Internal Rust reference changes are outside these counts.

## Method and parity

Both uv scales produced the full 3,440,812-byte CPython 3.14.5 reference catalog. The 1× input contains 567 releases and 58,721 artifacts. At 10× the process ingests the same release index ten times. The unchanged Ruff reporter emits the 146- and 1,460-file diff reports and both analyzer argument lists match the reference. Every timed process was checked against its full output digest.

Three-arm blocks shuffle all six arm orders on one pinned AMD EPYC Milan core. Each arm gets two warmups. Wall time includes startup, input, calculation, output and exit. Interval estimates resample complete permutation cycles 8,000 times. The admission criterion required a 95% lower speedup bound above 1 on 10× uv, a repeat on a separate core, and at most a 1% slowdown on the smaller uv and Ruff controls.

The initial runtime on main `2848e2a` independently measured 1.0172× [1.0058, 1.0206] on 10× uv. We repeated the study after the JSON encoder changed on `33fbb4d`. Rebuilding on `726b6a0` with the tuple file-ownership change gave identical split LLVM and ELF bytes in all three corresponding arms. An initial 10× trial overlapped an older Ruff run. Both series were excluded, and the uv trial was replaced before its times were examined.

The final candidate matched CPython on alias mutation, reassignments that raise partway through a tuple, eager defaults, Unicode and large integers. It keeps tuple-returning calls boxed. If a later argument raises, temporary buffered files flush in CPython’s order. The mutation matrix passed at `-O0`, `-O3`, and under AddressSanitizer/LeakSanitizer at `-O1`. The full catalog suite passed 66 cases, and Ruff’s CLI passed 13 cases.

[Raw timing blocks, executable hashes and operation counts](2026-10-24-named-tuple-keys.json.gz) · [Reproduction commands, hand rewrite and instrumentation](2026-10-24-named-tuple-keys-reproduction.tar.xz)
