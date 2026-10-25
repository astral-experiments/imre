> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff’s fixed regex in the conformance reporter

Imre now uses Ruff’s original verbose regex to find conformance annotations. It validates literal patterns at compile time and initializes each distinct pattern once per process. Rust’s `regex-automata` performs the searches. Imre retains Python’s character offsets and the distinction between empty and absent captures.

Scanning the suite with the regex was **2.67× faster than the previous Imre scanner in an ordinary release** and **2.48× faster with ThinLTO**. The new scanner took **1.984× and 1.792× Rust’s time**, respectively. Both builds met the 2× median target for every timed workload. Complete reports took 1.46–1.62× Rust’s time and live checkouts took 1.007–1.025×.

The ordinary scanner finished just 8.85 µs under twice the Rust median. The middle half of its samples ranged from 1.076 to 1.248 ms, versus 0.566 to 0.601 ms for Rust, after subtracting each implementation’s median preload. One-shot reports see a smaller change. Scanner preload rose by about 1.67 ms in both builds, and the unchanged ThinLTO report went from 32.98 to 33.56 ms. The other five one-shot report medians improved.

## Annotation scanner

Milliseconds per scan of 14,986 lines. Each timing process loads the corpus once and scans it 64 times. The table subtracts the corresponding zero-scan process median and divides by 64.

| Build | Upstream CPython | Typed CPython | Codon | Previous Imre | Imre | Rust | Imre/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | 14.471 | 15.596 | 4.794 | 3.021 | 1.130 | 0.570 | 1.984× |
| ThinLTO | 13.986 | 13.898 | 4.441 | 1.870 | 0.753 | 0.420 | 1.792× |

## Complete report

Milliseconds from process launch to exit, including reading the captured diagnostics and writing the report.

| Build and input | Upstream CPython | Typed CPython | Codon | Previous Imre | Imre | Rust | Imre/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary, Unchanged GitHub | 145.24 | 273.06 | 61.01 | 38.39 | 37.71 | 23.58 | 1.599× |
| Ordinary, Changed GitHub | 149.06 | 275.70 | 62.46 | 40.50 | 39.79 | 24.57 | 1.620× |
| Ordinary, Changed diff | 147.33 | 270.16 | 61.82 | 39.24 | 38.67 | 24.29 | 1.592× |
| ThinLTO, Unchanged GitHub | 147.07 | 274.26 | 62.00 | 32.98 | 33.56 | 22.49 | 1.492× |
| ThinLTO, Changed GitHub | 151.82 | 276.60 | 64.09 | 34.46 | 34.24 | 23.11 | 1.482× |
| ThinLTO, Changed diff | 146.60 | 270.68 | 61.91 | 33.40 | 32.96 | 22.57 | 1.460× |

## Repeated report

Milliseconds per report using four passes over preloaded inputs. Each column subtracts its matching zero-pass median before dividing by four.

| Build and input | Upstream CPython | Typed CPython | Codon | Previous Imre | Imre | Rust | Imre/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary, Unchanged GitHub | 50.43 | 169.51 | 32.18 | 20.61 | 18.55 | 10.38 | 1.787× |
| Ordinary, Changed GitHub | 52.68 | 174.06 | 35.56 | 22.23 | 19.94 | 11.55 | 1.726× |
| Ordinary, Changed diff | 51.63 | 168.99 | 34.45 | 21.20 | 19.23 | 11.07 | 1.736× |
| ThinLTO, Unchanged GitHub | 50.42 | 169.81 | 31.84 | 15.61 | 13.95 | 9.10 | 1.532× |
| ThinLTO, Changed GitHub | 53.69 | 173.96 | 36.54 | 16.88 | 15.33 | 10.16 | 1.509× |
| ThinLTO, Changed diff | 51.76 | 170.59 | 34.49 | 16.10 | 14.84 | 9.83 | 1.510× |

## Live checkout

Milliseconds from process launch to exit. Each reporter discovers the checkout, runs both ty executables, and writes the complete report.

| Build and input | Upstream CPython | Typed CPython | Codon | Previous Imre | Imre | Rust | Imre/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary, Unchanged GitHub | 982.92 | 1,318.04 | 967.44 | 895.06 | 895.70 | 874.48 | 1.024× |
| Ordinary, Changed GitHub | 1,010.87 | 1,354.98 | 1,001.42 | 919.25 | 915.97 | 896.94 | 1.021× |
| Ordinary, Changed diff | 1,017.65 | 1,357.01 | 1,010.59 | 920.00 | 917.67 | 900.58 | 1.019× |
| ThinLTO, Unchanged GitHub | 1,000.97 | 1,334.72 | 975.19 | 894.68 | 900.05 | 877.78 | 1.025× |
| ThinLTO, Changed GitHub | 999.72 | 1,326.46 | 981.77 | 907.64 | 898.03 | 891.53 | 1.007× |
| ThinLTO, Changed diff | 1,006.80 | 1,330.33 | 991.99 | 900.51 | 902.10 | 891.33 | 1.012× |

## Preload controls

Milliseconds with zero scan or report passes. These processes load the same inputs as their corresponding repeated workload.

| Build and input | Upstream CPython | Typed CPython | Codon | Previous Imre | Imre | Rust | Imre/Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary, scanner | 80.10 | 61.43 | 14.70 | 6.70 | 8.37 | 4.98 | 1.682× |
| Ordinary, Unchanged GitHub | 88.89 | 80.63 | 20.57 | 15.42 | 17.14 | 9.96 | 1.720× |
| Ordinary, Changed GitHub | 90.66 | 80.99 | 20.23 | 15.66 | 17.01 | 9.88 | 1.723× |
| Ordinary, Changed diff | 88.30 | 79.40 | 20.14 | 15.23 | 16.62 | 9.67 | 1.719× |
| ThinLTO, scanner | 78.84 | 60.28 | 14.60 | 6.47 | 8.13 | 4.86 | 1.672× |
| ThinLTO, Unchanged GitHub | 90.09 | 81.00 | 20.60 | 14.66 | 16.93 | 9.81 | 1.726× |
| ThinLTO, Changed GitHub | 89.35 | 80.70 | 20.13 | 14.51 | 16.35 | 9.86 | 1.659× |
| ThinLTO, Changed diff | 88.66 | 79.75 | 19.77 | 14.57 | 16.11 | 9.66 | 1.667× |

## CPU and allocation profile

These are ordinary-build Callgrind instruction counts and DHAT allocation requests per full scan. Each column subtracts an independent zero-pass process from the 16-pass process before division. The profilers ran outside the timing trials.

| Implementation | Executed instructions | Heap blocks requested | Heap bytes requested |
| --- | ---: | ---: | ---: |
| Previous Imre | 24,127,523 | 18,485.0 | 847,448 |
| Imre | 9,001,621 | 3,596.3 | 208,957 |
| Rust | 4,093,743 | 0.0 | −0.75 |

Both Rust processes requested 32 blocks. Their startup argument lengths account for a 12-byte difference over 16 passes. Peak live heap for the 16-pass process was 1,569,193 bytes for the previous scanner, 1,592,768 bytes for the regex scanner and 636,033 bytes for Rust.

The runtime reuses matching state, skips lines missing a required literal, and stores short capture spans inline. The compiler resolves known group names to numeric slots and tests capture presence without allocating a substring. The Rust reference borrows captured text from the input. [Runtime](../../crates/imre-runtime/src/objects/regexes.rs) · [Compiler](../../crates/imre-compiler/src/frontend/regex.rs)

## What ran

The new executable was built from `4eb4e4cd6aa81685bb42a7f1e95c1596901d3fc8` and the previous scanner from its stack parent `dad5a967fa7fa34cc34cddbc02e4ab35d147e0da`. Both modes ran on logical CPU 31 of the same AMD EPYC Milan machine. The tools were CPython 3.14.5, Codon 0.20.4, Clang 18.1.3 and Ohm rustc 1.98.1-dev (LLVM 22.1.8). Ordinary linking used GNU ld 2.42. ThinLTO used LLD 22.1.8. [Measured source change](https://github.com/astral-sh/imre/compare/dad5a967fa7fa34cc34cddbc02e4ab35d147e0da...4eb4e4cd6aa81685bb42a7f1e95c1596901d3fc8)

The corpus contains 14,986 lines from 146 files in the Python typing conformance suite. The scanner finds 1,329 annotations before the reporter applies its filters. The reporter uses the exact 567-byte pattern declaration from Ruff, including the comments inside the verbose string. [Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L75-L90) · [Typing suite](https://github.com/python/typing/tree/835b70f3f795f8e061b1f7621178515b0a77a0ad/conformance)

The upstream CPython scanner harness imports Ruff’s pattern and returns the same annotation fields as the typed scanner. Its report and checkout adapters run Ruff’s original report logic. Typed CPython, new Imre and native Codon run the typed port. The previous Imre build runs its original string scanner. The independent Rust program uses a handwritten scanner and borrows captured substrings. Codon 0.20.4 needs a benchmark adapter that checks the literal, removes verbose layout and expands Python’s Unicode whitespace for its native RE2 engine. [Reporter](../../examples/conformance_report/parse.py) · [Codon adapter](../codon_native/conformance_re.py) · [Rust reference](../reference/conformance_report)

The unchanged checkout compares ty 0.0.90 to itself. The changed checkout compares ty 0.0.60 to 0.0.90. Each of the six implementations gets one warm-up and 12 timed launches per workload. The six-round ordering repeats twice, giving every implementation the same positions and preceding neighbors. Each build mode has 1,008 timed processes across 14 workloads including the four zero-work controls. Timing begins before process launch and ends at exit. The harness checks exit status, stderr, stdout and the full output bytes for every sample.

The 2× target uses unrounded medians for every nonempty workload, including whole-process repeated runs and per-pass times after subtracting preload. The controls measure loading and startup on each implementation. The audit reconstructs the capture transcripts and complete reports independently from Ruff. [Benchmark harness](../conformance_regex_benchmark.py) · [Independent audit](../conformance_regex_audit.py)

## Output checks

All six implementations emitted the same 46,254-byte transcript on the real corpus. Its SHA-256 is `b83809751592f4403226089c1b49f9df8be700b3c872dd8b0f12a499ac5545e8`. They also matched the changed GitHub report, the changed diff and the unchanged GitHub report byte for byte. A separate 30,000-line seeded corpus checked Unicode character offsets, unmatched and empty captures, malformed tags and Python whitespace. Its 16,259 matches produced a 395,781-byte transcript.

The frozen compiler and runtime passed 309 Rust unit tests, 37 native differential suites at O0 and O3, and eight suites with AddressSanitizer and LeakSanitizer. The application validator checked 400 generated reports and 261 application, computation, stdout and error-path cases against upstream CPython. [Application validator](../conformance_validate.py) · [Supported regex syntax](../../docs/subset.md#fixed-regular-expressions)

## Replay

The timing archive contains the two source snapshots, build records, analyzer inputs, complete emitted reports and captures, and raw profiles. Its SHA-256 is `4328fbc67c9fce38572171adf0aa775f07bdb3abf7cac201eed105fd8fe9d3ff`. From the repository root, run the offline replay with CPython 3.14. [Study archive](2026-10-22-fixed-regex-study.tar.xz) · [Ordinary observations](2026-10-22-fixed-regex-ordinary.json.gz) · [ThinLTO observations](2026-10-22-fixed-regex-thinlto.json.gz) · [Strict audit results](2026-10-22-fixed-regex-audit.json)

```sh
study_dir=$(mktemp -d)
tar -xJf benchmarks/results/2026-10-22-fixed-regex-study.tar.xz -C "$study_dir"
(cd "$study_dir" && python3.14 -B replay.py)
```

The replay checks 623 indexed files, reconstructs the capture and report bytes from the pinned sources, recomputes all 2,016 timed observations’ medians and ratios, and checks the profile totals against the raw traces.

A separate native probe checks the Rust ABI directly against CPython, including all capture values and code-point spans after the caller releases its input. It reproduced 112,608 generated searches across 782 accepted patterns and another 15,007 searches on real lines and long or adversarial inputs. To run it with the same corpus, use the study directory from the command above. The checkout must contain measured commit `4eb4e4c`. Pass a local Rust toolchain name to `--toolchain`. [Native regex probe](2026-10-22-fixed-regex-oracle.tar.gz)

```sh
tar -xzf benchmarks/results/2026-10-22-fixed-regex-oracle.tar.gz -C "$study_dir"
python3.14 "$study_dir/native-regex-study/replay.py" \
  --checkout "$PWD" --corpus "$study_dir/inputs/corpus.txt" \
  --toolchain ohm --output "$study_dir/regex-replay.json"
```
