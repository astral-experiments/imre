# Reducing record and string allocations in uv’s catalog

Relar compiled the same streaming Python program against the same release index. At 10× its ordinary build made **73.5% fewer heap allocations** and executed **20.6% fewer instructions** than the previous build. Across all four inputs and both native build modes, complete runs took **1.11–1.22× Rust’s time**.

## Complete process time

Median wall time in seconds, including startup, input, selection and writing the catalog. Each input ran in 14 position-balanced rounds on one AMD EPYC Milan CPU. The first CPython column runs uv’s upstream program. The second runs the typed streaming Python that Relar compiled. Codon and Rust use the existing ports. Parentheses give Relar’s elapsed time divided by Rust’s in the same build mode.

| Input | Upstream CPython | Typed CPython | Codon | Relar | Rust | Relar ThinLTO | Rust ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1× plain | 0.993 | 2.622 | 0.550 | 0.128 (1.22×) | 0.105 | 0.095 (1.20×) | 0.079 |
| 1× gzip | 1.060 | 2.660 | 0.591 | 0.149 (1.17×) | 0.128 | 0.115 (1.16×) | 0.099 |
| 10× plain | 10.489 | 23.897 | 4.809 | 0.891 (1.21×) | 0.736 | 0.620 (1.20×) | 0.517 |
| 10× gzip | 11.196 | 24.298 | 4.954 | 1.071 (1.12×) | 0.957 | 0.787 (1.11×) | 0.709 |

The ordinary Relar build ran 7.10–11.77× faster than upstream CPython and 3.96–5.40× faster than Codon. 1 of 112 individual same-round ratios exceeded 2×. The largest was 2.15×.

## What changed

The catalog reads 58,721 artifacts in one copy of the release index. In Python it decodes every filename before looking up the target and the incumbent winning download. Profiling the unchanged Python counted 8,804 `Download` constructions at both scales. The 10× workload repeats the releases, so revisiting an incumbent usually discards the filename. [Selection loop](../../examples/python_catalog/catalog.py).

* **Delay single-use filename decoding.** When the compiler proves that a local is assigned and read once in the function, it can move default `unquote` across a loop guard to its use. It saves the immutable input string when the original expression is evaluated. A local that cannot be reassigned or cleared before use supplies its existing owner directly. Reading a field, indexing a list or calling a function that supplies the string still happens at the original point. The same rule handles `unquote(url.rsplit("/", 1)[-1])` with a known nonempty separator. The previous native build called the fused decoder 587,210 times on the 10× input. The new build called it 8,804 times. The fixed Rust reference continues to decode every filename before selection. [Rust selection loop](../reference/python_catalog/src/main.rs).
* **Pack records from typed JSON arrays.** After four individually allocated records, eligible arrays put new fixed-layout records in 512-byte blocks. Each record keeps its own reference count and releases its own fields when it dies. Records returned by sorting, slicing or indexing keep stable addresses. A survivor pins one block. On one copy of the index, 58,721 artifact allocations became 2,259 individual allocations and 5,969 blocks.
* **Reuse string storage within packed records.** For text of 33–256 UTF-8 bytes, the decoder can acquire a spare string owner and buffer. The last release may return an exclusive owner to its thread’s cache. A retained strong reference or weak observer prevents reuse. Idle payload capacity is capped at 64 KiB per thread, with at most 128 owners per size class. Bookkeeping for owners and bins uses additional memory.

A focused control cycled over 256 escaped URLs from the actual index. Across 24 paired runs, an all-kept guard took 1.6% longer than the previous build, with identical decoder, retain and release call counts. Keeping half, one in 32, or zero used 60.7%, 16.3%, and 12.6% of the previous elapsed time. A stable local can keep its most recently accepted filename alive until the next assignment or function exit. The memory held was independent of the number of skipped inputs.

## CPU, allocations and memory

These are whole-process ordinary builds on plain input. The 1× profiles were collected on a second host. The 10× profiles use the timing host. Callgrind counts simulated instructions. DHAT counts allocation requests and bytes over the process lifetime, and live heap at its high-water mark. MB uses decimal bytes in this table.

| Input | Metric | Relar before | Relar after | Rust | Change |
| --- | --- | ---: | ---: | ---: | ---: |
| 1× plain | Heap allocations | 579,045 | 232,597 | 341,071 | -59.8% |
| 1× plain | Cumulative allocated MB | 95.27 | 75.22 | 64.81 | -21.1% |
| 1× plain | Peak live heap MB | 12.14 | 12.24 | 9.20 | +0.8% |
| 1× plain | Instructions, millions | 916.6 | 779.1 | 636.8 | -15.0% |
| 10× plain | Heap allocations | 5,160,772 | 1,366,707 | 3,131,697 | -73.5% |
| 10× plain | Cumulative allocated MB | 772.83 | 548.18 | 446.15 | -29.1% |
| 10× plain | Peak live heap MB | 12.14 | 12.30 | 9.20 | +1.3% |
| 10× plain | Instructions, millions | 8,181.5 | 6,496.7 | 5,141.9 | -20.6% |

On 10× plain input the site counts reconcile exactly. Relar eliminated 1,156,812 filename owner and buffer allocations, 504,930 record allocations and 2,132,342 copied JSON string allocations. The cache added 19 bookkeeping allocations. Reading physical rows and materializing the strings from `splitlines()` together account for 398,481,334 bytes, or 72.7% of the remaining allocation traffic. The optimized trace attributes 38.5% of process instructions, including callees, to typed JSON decoding and 17.7% to dictionary lookups. Relar and Rust each executed 760.7 million exclusive instructions in the underlying JSON scanner, `skip_to_escape`.

Median peak resident memory in MiB, measured by `wait4` for the actual process in each timing round.

| Input | Upstream CPython | Typed CPython | Codon | Relar | Rust | Relar ThinLTO | Rust ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1× plain | 119.0 | 30.8 | 42.8 | 15.2 | 11.2 | 15.4 | 11.4 |
| 1× gzip | 119.3 | 31.6 | 42.8 | 15.6 | 11.3 | 15.6 | 11.3 |
| 10× plain | 990.2 | 31.4 | 42.8 | 15.4 | 11.1 | 15.7 | 11.5 |
| 10× gzip | 990.4 | 32.5 | 42.8 | 15.8 | 11.4 | 15.8 | 11.4 |

In a separate 38 MiB workload with 1,024 arrays of 64 records, keeping one record per array raised median peak RSS from 4.0 to 4.5 MiB. Discarding every record stayed at 3.25 MiB and retaining a string per array stayed at 3.5 MiB. After warming one string size class, one escaped record and its string added 712 live allocator bytes at 64, 4,096 and 32,768 rows. With all records released, that class held 23,600 idle bytes at both larger sizes, including after a late decode error. The worker released all measured bytes on thread exit. General-purpose string recycling also cut catalog allocations in a prototype, but added 7.1% instructions on a repeated small JSON document. The implemented cache is scoped to packed record decoding and cleanup.

## Comparison with the previous build

A separate run interleaved the previous Relar executable, the optimized executable and Rust, in ordinary and ThinLTO modes. Each input ran in six position- and neighbor-balanced rounds. The reductions below compare medians from this run.

| Input | Ordinary before → after (ms) | Time reduction | ThinLTO before → after (ms) | Time reduction |
| --- | ---: | ---: | ---: | ---: |
| 1× plain | 147.3 → 133.0 | 9.7% | 114.4 → 98.8 | 13.7% |
| 1× gzip | 162.9 → 149.1 | 8.5% | 128.4 → 113.9 | 11.3% |
| 10× plain | 1,020.1 → 871.3 | 14.6% | 747.8 → 617.5 | 17.4% |
| 10× gzip | 1,204.5 → 1,072.1 | 11.0% | 934.3 → 809.3 | 13.4% |

## Correctness and reproduction

All four inputs produced the same 6,098 entries and 3,440,812 bytes, matching upstream CPython byte for byte. The 10× gzip input contains ten concatenated members. A separate 26-winner corpus changes winners throughout the stream and crosses the record and string size boundaries. It passed in plain and gzip form across eight configurations, including an ASan/LSan Relar build. Both ordinary and instrumented builds also reproduced the previous runtime’s complete output and diagnostics on 836 JSON cases and 170 nested-array cases at O0 and O3.

The measured compiler and runtime are at `db44bcc2e9d0f3dca291ed141d7b742227ab44b4`. The final compiler at `db2a6f876e94fa136450ad3efe56c110e376b17d` adds the stable-local correction. Its catalog LLVM is identical. Linking the corrected LLVM with the measured runtime archives, original temporary input names and original flags reproduced all six ordinary, ThinLTO and ASan catalog CLI and compute executables byte for byte. Independent fresh builds had the same executable code, with differences in build metadata and an ASan filename. The freshly compiled catalog produced the same output on each of the four inputs. The old binaries are from the previous study at `34abb56ab902ff8f1ea014024dfa18a814679d3c`, whose implementation was merged in `231f19a21cdb52a93cc5ecbab86644d233ba4ff9`. All Python, Codon and Rust application sources have the same digests across both studies. [Previous catalog measurements](2026-10-22-catalog-hot-paths.md).

The raw matrix contains 392 timed processes and 28 warmups. The study archive includes all 144 timed before-and-after processes and their 24 warmups, profile traces, build identities, control measurements and correctness receipts. [Raw matrix](2026-10-22-catalog-ownership-measurements.json.gz) · [Audit](2026-10-22-catalog-ownership-audit.json) · [Profiles and paired measurements](2026-10-22-catalog-ownership-study.tar.gz).

```sh
source_dir=$(mktemp -d)
git archive db44bcc2e9d0f3dca291ed141d7b742227ab44b4 | tar -x -C "$source_dir"
study_dir=$(mktemp -d)
tar -xzf benchmarks/results/2026-10-22-catalog-ownership-study.tar.gz -C "$study_dir"
python3.14 "$study_dir/verify-study.py" \
  benchmarks/results/2026-10-22-catalog-ownership-measurements.json.gz \
  benchmarks/results/2026-10-22-catalog-hot-paths-measurements.json.gz \
  --source-root "$source_dir"
```

SHA-256 of the compressed matrix is `d3f564bf7cf3db93a77eb213c43f812daf5a3282267acc4935ca7397c73323e1`. SHA-256 of the profile and paired-measurement archive is `1852cbae6b57000ae0333c6194e3b5c86964acbd4ced40921cd0dae43ffb4772`.
