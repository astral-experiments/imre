> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Reducing overhead in uv’s streaming download catalog

One release index contains 58,721 artifacts in 567 NDJSON records. Ten copies contain 587,210 artifacts and 198,704,170 uncompressed bytes. Each run reads its input, selects the downloads, and writes the same 6,098-entry, 3,440,812-byte catalog. The ten-copy gzip input has ten concatenated members. A separate correctness fixture puts a distinct winner in every member.

## Complete process time

Median wall time in seconds on one AMD EPYC CPU. Each input ran in 14 position-balanced rounds. The two CPython columns run the pinned upstream implementation and the typed streaming program that Imre compiles. Codon runs its existing port, and Rust runs its hand-written reference. The ordinary and ThinLTO columns pair matching native build modes. All application and reference sources are identical to the previous study.

| Input | Upstream CPython | Typed CPython | Codon | Imre | Rust | Imre ThinLTO | Rust ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1× plain | 1.056 | 2.627 | 0.555 | 0.152 (1.39×) | 0.109 | 0.114 (1.38×) | 0.082 |
| 1× gzip | 1.059 | 2.647 | 0.593 | 0.168 (1.32×) | 0.127 | 0.129 (1.31×) | 0.099 |
| 10× plain | 10.160 | 23.620 | 4.478 | 1.030 (1.40×) | 0.736 | 0.745 (1.46×) | 0.512 |
| 10× gzip | 10.779 | 24.128 | 4.871 | 1.221 (1.29×) | 0.945 | 0.919 (1.31×) | 0.701 |

Imre took **1.29–1.46× Rust’s time**. Upstream CPython took 6.32–13.64× as long as Imre. All 112 individual same-round ratios were below 2×, with a maximum of 1.80×.

## Before and after

A separate run interleaved the previously published Imre binaries, the optimized binaries and the same Rust references on the same CPU. Each entry compares medians from six position- and neighbor-balanced rounds.

| Input | Ordinary before → after (ms) | Time reduction | ThinLTO before → after (ms) | Time reduction |
| --- | ---: | ---: | ---: | ---: |
| 1× plain | 177.7 → 146.3 | 17.7% | 127.1 → 108.9 | 14.3% |
| 1× gzip | 201.6 → 168.3 | 16.5% | 146.4 → 129.7 | 11.4% |
| 10× plain | 1,310.7 → 1,026.8 | 21.7% | 877.1 → 742.3 | 15.4% |
| 10× gzip | 1,515.0 → 1,222.2 | 19.3% | 1,073.6 → 923.6 | 14.0% |

## Peak resident memory

Median peak RSS in MiB, measured for each fresh process. Repeating the index keeps the selected output fixed.

| Input | Upstream CPython | Typed CPython | Codon | Imre | Rust | Imre ThinLTO | Rust ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1× plain | 119.0 | 30.3 | 42.5 | 15.2 | 11.2 | 15.4 | 11.2 |
| 1× gzip | 119.3 | 31.1 | 42.8 | 15.6 | 11.3 | 15.6 | 11.3 |
| 10× plain | 990.2 | 31.6 | 42.5 | 15.6 | 11.3 | 15.8 | 11.6 |
| 10× gzip | 990.4 | 32.2 | 42.6 | 15.7 | 11.4 | 15.7 | 11.4 |

## CPU and allocations

We profiled whole ordinary 10× plain processes with Callgrind and DHAT, and checked their output against the same upstream catalog. The host restricted hardware performance counters. Callgrind counts executed instructions, and DHAT measures cumulative allocation traffic and live heap. The wall times above come from separate uninstrumented processes.

| 10× plain | Imre before | Imre after | Rust |
| --- | ---: | ---: | ---: |
| Executed instructions | 10.552 billion | 8.181 billion | 5.141 billion |
| Heap allocations | 8,073,496 | 5,160,772 | 3,131,697 |
| Cumulative allocated bytes | 1,331.4 MB | 772.8 MB | 446.2 MB |
| Peak live heap | 11.58 MiB | 11.58 MiB | 8.77 MiB |

The allocation profiles attribute 1,162,300 fewer blocks to URL splitting and decoding, 1,148,120 to typed JSON, 581,312 to record creation outside those paths, and 20,992 to buffered line reads. Together these account for the full reduction of 2,912,724 blocks. Line reads alone fell from 628.15 MB to 199.19 MB in cumulative allocations. The separate `splitlines` operation still allocated 199.29 MB, while its inclusive instruction count fell from 713.50 million to 294.96 million.

The original JSON decoder made a dedicated depth pass over every release before reading it again. The optimized decoder uses the parser’s nesting limit for ordinary records. When a raw number, a skipped container or a parse error requires an explicit check, the number of opening delimiters can establish an upper bound before a full scan. Documents up to 128 bytes already fit the depth limit. A bounded stack table shares the short, repeated platform and variant names within each release. Documents smaller than the table bypass it.

The original artifact loop allocated 587,210 temporary platform/variant tuples. Reusing a uniquely owned loop tuple eliminated 581,312 of those allocations. Aliased tuples, including keys retained by dictionaries, keep their values. The compiler also passes the final URL component directly to `unquote`. The reader reuses a bounded buffer for lines spanning several reads, and `splitlines` handles common single ASCII lines in one scan.

```python
for artifact in sorted(record.artifacts, key=artifact_key):
    filename = unquote(artifact.url.rsplit("/", maxsplit=1)[-1])
    labels = (artifact.platform, artifact.variant)
    target = selection.targets.get(labels)
```

The complete process executed 22.5% fewer instructions and requested 36.1% fewer heap allocations. The largest remaining exclusive instruction count was in `<serde_json::read::SliceRead>::skip_to_escape`, at 761 million instructions (9.3% of the process).

A separate 1× ordinary profile on a second host counted 579,044 Imre heap allocations and 341,071 Rust allocations. DHAT counted 118,982 uncached string owners. The input contains 117,442 long URL and digest strings and 1,540 long variant strings. Imre also allocated 58,721 artifact records and 58,721 owners for decoded filenames. Counting just the URL and digest owners, artifact records, and filename owners gives 234,884 blocks, compared with a net difference of 237,973 across all sites. Rust stores string fields inline in its artifact vector and moves the filename string into the result. Both engines separately allocated 58,721 filename backing buffers totaling 4,494,790 bytes, and their shared JSON string scanner executed exactly 70,696,742 instructions in each.

## Cross-workload check

The first cache implementation initialized 64 slots even for a 20-byte configuration. That made the default-heavy control 69% slower than `main`. With the final compiler and runtime, seven small and medium controls took 0.88–0.99× the original runtime’s time. The controls cover numeric records, independent and repeated strings, and missing defaults. Each measured process decoded 385,500 tiny records or 89,950 medium records and checked its output against CPython. On 10× plain input, a separate ten-pair comparison of the intermediate and final catalog executables gave a median within-pair ratio of 0.9965×. The corrected build was faster in seven pairs.

The conformance reporter compared 146 test modules and computed its complete changed-diff report four times per invocation. In 12 balanced ordinary-build rounds on a separate devbox, the median was 98.56 ms before, 99.07 ms after, and 57.74 ms for Rust. The optimized build was faster in 8 of 12 pairs. All 39 executions, including warmups, reproduced the expected 120,737-byte diff and checksum.

## Replaying the comparison

The compiler and runtime passed 297 unit tests. The five affected Python suites passed all 468 checks in ordinary and sanitizer builds at optimization levels 0 and 3. Catalog validation covered 748 executions across the ordinary, ThinLTO and sanitizer modes. The typed JSON differential also matched the original native values and exact error messages on 836 inputs, and 556 sanitizer checks exercised the nesting boundary. Compiler Clippy passed. Strict workspace Clippy reported two pre-existing diagnostics in TOML and subprocess code.

The full matrix recorded 392 measured processes and 28 warmups. Its audit checked the retained files on the measuring host and independently reproduced the statistics from the compressed record. The study archive includes the original Callgrind and DHAT traces, all 168 native comparison runs, validation digests, and the conformance measurements. The ordinary, ThinLTO, and sanitizer catalog builds used the same optimized source. [Measurements](2026-10-22-catalog-hot-paths-measurements.json.gz) · [Audit](2026-10-22-catalog-hot-paths-audit.json) · [Profiles and paired runs](2026-10-22-catalog-hot-paths-study.tar.gz).

```sh
git fetch --no-tags origin 34abb56ab902ff8f1ea014024dfa18a814679d3c
source_dir=$(mktemp -d)
git archive 34abb56ab902ff8f1ea014024dfa18a814679d3c | tar -x -C "$source_dir"
python3.14 benchmarks/stream_catalog_audit.py benchmarks/results/2026-10-22-catalog-hot-paths-measurements.json.gz \
  --source-root "$source_dir" --sha256 cd67f3fe3e79d0d4d4aa522d86cebe45d54c039b54fd36968b8bc227009d8e84
```

The paired comparison can also be replayed locally. The SHA-256 of its archive is `ab965470ebb67934dafa66ee99c262b79a5449693e6f6a10253bf5bf1f430787`.

```sh
study_dir=$(mktemp -d)
tar -xzf benchmarks/results/2026-10-22-catalog-hot-paths-study.tar.gz -C "$study_dir"
python3.14 "$study_dir/verify-paired.py" "$study_dir/paired.json" \
  benchmarks/results/2026-10-22-streaming-catalog-measurements.json.gz \
  benchmarks/results/2026-10-22-catalog-hot-paths-measurements.json.gz
```
