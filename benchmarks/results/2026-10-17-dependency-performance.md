> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Dependency planner: CPU and allocation optimization

The unchanged Python dependency planner now runs about 1.5× faster than the preserved buffered Relar baseline on both large workloads. With ThinLTO and matched LLVM optimization budgets, Relar is approximately level with the arbitrary-precision Rust reference on the layered graph. The wide graph remains 46% slower than that Rust reference. Checked-i64 Rust is faster on both, with a narrower numerical contract.

These results use implementation revision `2db836788546b55cf68969aaadd8539010b4358f`. The six Python package files and Rust reference algorithm are unchanged. The compiler and independent runtime remain Rust implementations, with no CPython fallback.

## End-to-end results

Each cell is the median of 30 fresh-process runs in milliseconds. The main comparison enables ThinLTO for both new Relar and Rust, with LLVM import and inline budgets of 1,000. The preserved Relar baseline uses its original ordinary linking configuration.

| Graph | Tasks | CPython 3.14.5 | Previous Relar | Relar ThinLTO | Rust BigInt ThinLTO | Rust checked-i64 ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 20.317 | 3.407 | 3.241 | 3.311 | 3.054 |
| Layered | 2,048 | 31.774 | 8.424 | 6.589 | 6.518 | 5.194 |
| Layered | 16,384 | 121.822 | 50.509 | 33.021 | 33.393 | 20.904 |
| Wide | 128 | 20.239 | 3.308 | 3.194 | 3.233 | 2.988 |
| Wide | 2,048 | 31.086 | 6.694 | 5.350 | 4.787 | 4.245 |
| Wide | 16,384 | 117.482 | 33.540 | 22.344 | 15.296 | 11.588 |

At 16,384 tasks, Relar takes 34.6% less time on the layered graph and 33.4% less on the wide graph than the previous Relar build. It is 3.69× and 5.26× faster than CPython. Its ratios to Rust BigInt are 0.989× and 1.461×; its ratios to checked-i64 Rust are 1.580× and 1.928×. Lower ratios indicate less elapsed time.

The layered Relar/Rust BigInt difference is small: their interquartile ranges are 32.758–33.319 ms and 33.201–33.772 ms. Treat this as competitive performance on this workload, not a demonstrated general lead over Rust. For the wide graph, the corresponding ranges are 22.124–23.015 ms and 14.971–15.929 ms. These observed ranges are not confidence intervals.

Ordinary linking also improves performance. The separate ordinary-link comparison uses the original Rust release reference binaries:

| Graph | Tasks | CPython 3.14.5 | Previous Relar | Relar | Rust BigInt | Rust checked-i64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Layered | 128 | 20.357 | 3.405 | 3.294 | 3.348 | 3.055 |
| Layered | 2,048 | 31.526 | 8.417 | 6.893 | 6.888 | 5.336 |
| Layered | 16,384 | 122.460 | 50.718 | 35.671 | 36.449 | 23.398 |
| Wide | 128 | 20.388 | 3.312 | 3.205 | 3.261 | 3.029 |
| Wide | 2,048 | 30.431 | 6.559 | 5.573 | 4.953 | 4.273 |
| Wide | 16,384 | 118.339 | 33.604 | 24.872 | 17.054 | 13.082 |

The ordinary build reduces large-workload elapsed time by 29.7% and 26.0%. ThinLTO remains optional: compiling and linking this example took 0.800 seconds ordinarily and 4.974 seconds with ThinLTO. Those are single build observations, excluding construction of the compiler and runtime. The unstripped executables are 2,671,648 and 2,478,088 bytes respectively.

## What the profiles changed

Callgrind identified repeated parsing, temporary tuple construction, container access, and output formatting work. DHAT identified the allocation sites. We made the following cumulative changes:

1. Parse ordinary signed decimal input directly into the small-integer representation. Separators and larger integers retain the existing checked arbitrary-precision path. This removes 165,021 allocation blocks on the layered input and 49,155 on the wide input.
2. Replace eligible local tuples with separate owned component slots. Arguments and elements still execute once, left to right; whole-value escapes retain the boxed representation. This removes 65,536 allocation blocks and 2,883,584 requested bytes on each input.
3. Store short UTF-8 strings inline within their reference-counted owner using the already-pinned `compact_str` crate. This removes 73,040 and 32,777 blocks; split tokens still have individual owners.
4. Decode list indices before borrowing storage, isolate cold integer/error paths, and enable optional cross-language ThinLTO. The larger import/inlining budgets let LLVM optimize through runtime calls. Negative indexing, bounds checks, reference ownership, and arbitrary-precision fallback remain intact.
5. Embed up to four fixed fields in their owner, retaining vector storage for larger records. This removes another 16,386 blocks per workload. The shared representation makes sequence headers larger, so cumulative requested bytes increase by 524,480 at this stage even though peak live bytes fall by 130,944.
6. Write string, boolean, `None`, and small-integer bytes directly into the existing output buffer; small integers use an `itoa` stack buffer. This removes roughly 19.6 million and 18.3 million executed instructions relative to the preceding stage. The existing output mutex and flush/error behavior remain in place.

The compiler already borrowed unchanged parameters and direct local operands before this work. Those are not new optimizations. Small-integer printing also already avoided BigInt conversion; the new output improvement removes formatting machinery.

| Cumulative Relar stage | Layered instructions | Wide instructions | Layered allocation blocks | Wide allocation blocks |
| --- | ---: | ---: | ---: | ---: |
| Buffered baseline | 356,175,765 | 237,559,248 | 553,880 | 311,438 |
| Small decimal parsing | 293,018,048 | 219,984,344 | 388,859 | 262,283 |
| Local tuple replacement | 270,953,648 | 197,921,860 | 323,323 | 196,747 |
| Inline short strings | 257,919,027 | 190,168,442 | 250,283 | 163,970 |
| Decoded access and tuned ThinLTO | 234,279,179 | 168,161,587 | 250,283 | 163,970 |
| Inline fixed fields | 224,783,154 | 163,517,609 | 233,897 | 147,584 |
| Direct byte output | 205,147,254 | 145,225,371 | 233,897 | 147,584 |

The final build removes about 42%/39% of executed instructions and 58%/53% of allocation blocks. Total requested bytes fall from 55,350,105 to 51,543,811 on the layered graph and from 12,926,081 to 10,078,143 on the wide graph. Peak live bytes fall from 6,350,602 to 6,055,808 and from 5,425,066 to 5,130,272.

These are cumulative profiles. The decoded-access stage includes both runtime changes and tuned ThinLTO, so it is not an isolated measurement of either change. Hardware-counter profiling was unavailable on this host, so CPU attribution uses Callgrind instruction/call traces. Callgrind counts executed instructions, not CPU cycles or elapsed-time shares. DHAT reports requested allocations, not allocator-reserved memory or process RSS. Instrumented durations are excluded from the timing results.

## What still separates Relar from Rust

The wide graph stresses the ready-queue heap. Relar still pays for tagged integer decoding, Python-style negative-index normalization, checked bounds, and mutable-container borrowing around its heap operations. Rust expresses the same algorithm using native indices and direct vectors, giving its compiler more information to eliminate work. Inlining makes those operations visible to LLVM but does not establish all the range and aliasing proofs needed to remove them.

In the exact measured wide-graph binary, the scheduler accounts for 54,111,356 self instructions, including 42,689,585 in the normal inlined `pop_ready` region. That region executes 581,130 signed-index normalization and bounds-check sequences, plus 4,896,700 immediate tag-test instructions; mutable-borrow guards and updates also recur in its hot blocks. These counts locate work to investigate. They do not establish that every check can be removed or measure its elapsed-time share.

The matched-budget Rust BigInt reference executes 196,808,173 instructions on the layered graph and 81,616,504 on the wide graph, with 306,204 and 147,562 allocation blocks. Relar's wide allocation count is already close, although it requests 10,078,143 bytes versus Rust's 7,219,551. Together with the heap-loop checks, this suggests further gains need lower representation and access overhead; allocation counts alone do not explain the elapsed-time gap. Checked-i64 Rust executes 135,349,106 and 56,356,046 instructions, but cannot represent all supported program values.

The layered graph also builds a long critical-path string through repeated fresh prefix copies. Both Python and Rust retain that algorithm. Copy instruction counts are especially unsuitable as wall-time percentages. Future work should pursue proved nonnegative indices, range-aware integer specialization, and more concrete container layouts while preserving fallback behavior. Removing checks without those proofs would change the contract.

## Correctness and measurement scope

Both comparisons pass 135 independent planner-oracle cases for CPython, both Relar builds, and Rust BigInt. Checked-i64 Rust passes the 121 representable cases; the other 14 are listed explicitly. Ten additional boundary cases check integer limits and overflow behavior. Every timed execution must match the complete expected output, emit no stderr, and exit successfully.

Formatting, Clippy, nine runtime unit tests, the ordinary O0/O3 native suites, the ThinLTO O3 suites, and ten ThinLTO CLI checks pass. The five native suites also pass with AddressSanitizer/LeakSanitizer for both an instrumented Rust runtime and generated code, using ordinary O1 and ThinLTO O3 builds. The nine runtime unit tests pass with AddressSanitizer. These local builds instrument our runtime; they do not rebuild and instrument the entire Rust standard library. CI additionally covers the normal native suites, ThinLTO, generated-code sanitizers, and independent planner oracles.

Measurements ran outside ptrace on CPU 0 of an AMD EPYC-Milan devbox, with builds and profilers paused. Each of six workloads has two warmup rounds and 30 measured rounds. Ten Williams orders repeat three times, balancing execution positions and directed adjacent pairs within each round. Cross-round predecessors are not balanced. Timings include process startup, input/output pipes, parsing, scheduling, reporting, and exit. CPython imports the unchanged package. Compilation, validation, warmups, and output checking are excluded.

The compiler/runtime use Ohm Rust 1.98.1-dev with LLVM 22.1.8; generated IR enters through Clang 18.1.3. ThinLTO uses LLD 22.1.8. Relar and the matched Rust references use import/inlining budgets of 1,000; Rust also enables release ThinLTO explicitly. Ohm's experimental build defaults were disabled. CPython is 3.14.5 with its JIT disabled; its recorded build configuration indicates PGO and LTO. The shared host still introduces noise, especially for small programs dominated by startup.

The Rust reference implements the same scheduling/report algorithm in a different source language. BigInt preserves arbitrary-precision program values; checked-i64 deliberately rejects overflow. Rust uses native indices where the algorithm establishes valid container sizes. These measurements establish neither general Rust parity nor a result for C.

## Evidence and reproduction

- [Matched ThinLTO comparison](2026-10-17-dependency-performance-lto.json): raw samples, full expected outputs, execution orders, validation, build settings, and tool/source/artifact hashes.
- [Ordinary-link comparison](2026-10-17-dependency-performance-normal.json): the same evidence against the original ordinary Rust references. Their historical build record includes source/binary hashes and tool versions, but predates recording Rust tool executable hashes.
- [Instruction and allocation profiles](2026-10-17-dependency-performance-profiles.json): cumulative stages, Rust controls, full-output checks, and hashes of preserved local traces. Raw Callgrind/DHAT traces remain in ignored local directories.
- [Compiler/runtime build record](2026-10-17-dependency-performance-build.json): exact implementation revision, source hashes, build commands, toolchain, and frozen artifacts.
- [Previous baseline record](2026-10-17-dependency-planner-comparison.json): provenance for the preserved buffered Relar executable.

The [subset documentation](../../docs/subset.md) describes the optional ThinLTO build recipe. Build the Rust controls with `CARGO_PROFILE_RELEASE_LTO=thin` and `benchmarks/reference/dependency_planner/build.py --llvm-budget 1000`; use the required `--toolchain ohm` option on an Ohm-configured development machine. Its [README](../reference/dependency_planner/README.md) documents the ordinary build and integer contracts.

Run `benchmarks/dependency_compare.py` with the preserved baseline and record, the new compiler/runtime, both Rust binaries and their build record, and `--samples 30 --warmups 2 --cpu 0`. Add `--lto-linker /path/to/ld.lld --target x86_64-unknown-linux-gnu` for the ThinLTO comparison. The supplied runtime must contain compatible bitcode. The linker filename must preserve the `ld.lld` invocation flavor. The JSON records the normalized command and exact flags; `benchmarks/dependency_profile.py` profiles the same captured inputs without regenerating them.
