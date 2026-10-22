# Streaming uv’s Python download catalog

The Relar executable reads the pinned python-build-standalone index directly as plain NDJSON or gzip, selects the downloads, and writes the complete catalog. The 1× fixture has 567 releases, 58,721 artifacts, and 19,870,417 uncompressed bytes. The 10× fixture repeats those same rows, for 198,704,170 bytes. Repeating rows holds the winning downloads and the size of the output fixed while increasing the amount of input each implementation must process.

**All eight time comparisons met the 2× Rust target.** Relar took 1.495–1.780× Rust’s time across sizes, encodings, and build modes. Upstream CPython took 5.41–11.53× as long as Relar. Relar took 0.20–0.33× Codon’s time and used 15.4–15.9 MiB of peak resident memory.

## Complete process time

Median wall-clock seconds on one Linux CPU. Each observation includes startup, all reads and decompression, release decoding, artifact URL sorting and selection, final sorting and JSON encoding, output publication, and exit. The two CPython columns distinguish the pinned upstream transformation from the shared typed streaming program.

| Input | Upstream CPython | Typed CPython | Codon | Relar | Rust | Relar ThinLTO | Rust ThinLTO |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1× plain | 1.002 | 2.619 | 0.552 | 0.176 | 0.106 | 0.128 | 0.079 |
| 1× gzip | 1.058 | 2.654 | 0.587 | 0.196 | 0.126 | 0.146 | 0.098 |
| 10× plain | 10.152 | 23.739 | 4.502 | 1.289 | 0.724 | 0.880 | 0.497 |
| 10× gzip | 11.086 | 24.294 | 4.926 | 1.518 | 0.952 | 1.086 | 0.705 |

Ten times the gzip input took 10.48× as long for upstream CPython, 9.15× for typed CPython, 8.39× for Codon, 7.75× for ordinary Relar, and 7.53× for ordinary Rust. All four workloads write the same number of selected downloads.

## Peak resident memory

Median peak RSS in MiB for each fresh child. A small native monitor uses `wait4` to collect each child’s high-water mark. GNU time cross-checks all 28 warmups. The retained winners, final JSON object, and rendered output are part of the process footprint.

| Input | Upstream CPython | Typed CPython | Codon | Relar | Rust | Relar ThinLTO | Rust ThinLTO |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1× plain | 119.0 | 31.5 | 42.8 | 15.4 | 11.2 | 15.7 | 11.5 |
| 1× gzip | 119.3 | 31.9 | 42.5 | 15.9 | 11.3 | 15.8 | 11.3 |
| 10× plain | 990.2 | 31.5 | 43.1 | 15.4 | 11.3 | 15.6 | 11.3 |
| 10× gzip | 990.3 | 32.0 | 43.1 | 15.7 | 11.4 | 15.7 | 11.4 |

With ten times the input, upstream CPython used 8.32× as much peak memory for plain NDJSON and 8.30× for gzip. The corresponding Relar ratios were 0.996× and 0.992× ordinarily, and 0.996× and 0.992× with ThinLTO. On gzip the typed CPython, Codon, and ordinary Rust peaks changed by factors of 1.003×, 1.015×, and 1.011×, respectively. This stress case holds retained winners fixed. A catalog selecting more unique downloads needs storage for its additional output.

| Input | Relar / Rust time | ThinLTO time | Relar / Rust peak RSS | ThinLTO peak RSS |
|---|---:|---:|---:|---:|
| 1× plain | 1.665× | 1.623× | 1.381× | 1.365× |
| 1× gzip | 1.548× | 1.495× | 1.404× | 1.404× |
| 10× plain | 1.780× | 1.773× | 1.355× | 1.376× |
| 10× gzip | 1.594× | 1.540× | 1.379× | 1.378× |

Across the 112 individual same-round native time comparisons, 111 were below 2×. Round 12 on ordinary 10× plain input reached 2.043×. Its fourteen-sample ratio of medians was 1.780×.

## Programs and inputs

The upstream CPython adapter supplies a whole-file response to uv’s unchanged finder and renderer. The typed CPython, Codon, and Relar programs stream the same Python algorithm. The independent Rust reference also streams and uses the same 512-entry limit on successfully normalized platform/variant pairs. Each engine decodes the original records, sorts every release’s artifact URLs, applies the per-release musl cutoff, and preserves first-winner ties. The 1× input contains 246 distinct raw platform/variant pairs.

Both gzip sizes include decompression in the measured process. The shipped 2,844,194-byte gzip fixture is the 1× input. The 10× archive concatenates ten copies into 28,441,940 bytes. A separate preflight uses ten different winning records, one in each member, to detect a reader that stops early. Every measured run and warmup must reproduce the upstream 6,098-entry catalog byte for byte, including its trailing newline. Its 3,440,812 bytes have SHA-256 `753cd906307c1b4f830e9348703e0110ba38fddd97293780f146f78f0ed9a3f6`. The [catalog guide](../../examples/python_catalog/README.md) links the pinned source, input, and exact CLI.

Codon 0.20.4 runs natively and specializes the shared reader for its plain and gzip handles. Its comparison covers the real index and ordinary concatenated members. Its native gzip reader accepts a plain file named `.gz`, can stop at zero padding between members, and reports some corrupt or truncated files as `OSError`. The [validation suite](../catalog_validate.py) records these differences.

## Method and verification

Fourteen counterbalanced orders per input give each of the seven configurations two appearances in every position and balance immediate predecessors. The five native artifacts are ordinary and ThinLTO pairs of Relar and Rust plus one portable Codon binary. There are 392 measured child processes and 28 warmups. The cache is warm, engines run serially, and no startup or decompression control is subtracted. Filesystem preparation and output verification happen outside the measured interval.

Measurements ran on one core of an AMD EPYC-Milan Processor (CPU 31), with Linux 6.8.0-147-generic, CPython 3.14.5 (JIT disabled), Codon 0.20.4, Clang 18, and Ohm Rust 1.98.1-dev. The reader was built at `347fc67` and the catalog at `f9b28e8`. [Reader PR](https://github.com/astral-sh/relar/pull/87) · [Catalog PR](https://github.com/astral-sh/relar/pull/88)

The initial 10× plain pilot measured 2.101× Rust’s time ordinarily and 2.126× with ThinLTO. In the 1× ordinary profile, Relar made 525,190 allocations attributed to string splitting and 365,969 to constructing records. The 10× ordinary profile counted 16.274 billion instructions and 1.746 GB of cumulative heap allocations for Relar, versus 7.288 billion instructions and 0.572 GB for Rust. Reusing normalized targets, formatting the build once per release, and constructing only winning records reduced the pilot time ratios to 1.779× and 1.709×. Relar and Rust use the same cache limit and the same zlib-rs 0.6.8 inflate backend. The pilot predates the final reader build. Only the frozen full study supplies the eight target comparisons above.

The compiler and runtime passed 296 unit tests. The four reader-related suites passed 309 checks both ordinarily and under ASan/LSan. The final catalog passed 66 cases across 193 upstream CPython, typed CPython, and Relar executions. The instrumented catalog repeated that matrix and passed another 14 cases across 41 executions of the preloaded computation. Cases cover more than 512 target labels, later winning records, corrupted gzip, Unicode errors, and preserving an existing output when input validation fails. Workspace formatting and compiler Clippy passed. Strict workspace Clippy reports two inherited findings in TOML and subprocess code.

The [raw measurements](2026-10-22-streaming-catalog-measurements.json.gz) retain every time, peak RSS, output hash, warmup, command, build identity, and engine order. The [independent audit](2026-10-22-streaming-catalog-audit.json) checked 320 retained file identities on the measuring host and reproduced every median and ratio. The [validation evidence](2026-10-22-streaming-catalog-validation.json) and [exploratory profiles](2026-10-22-streaming-catalog-profile.json) describe the tested configurations and the optimization pilot.

The [benchmark guide](../README.md#streaming-python-download-catalog) gives the build and measurement commands. From the commit that published these results, this command checks the recorded source identities and shipped fixture, then recomputes the statistics using Python’s standard library.

```sh
python3.14 benchmarks/stream_catalog_audit.py benchmarks/results/2026-10-22-streaming-catalog-measurements.json.gz \
  --sha256 a6d3818c4cf92b8681acf5bde0a2fa4276e40dfef5f6a2dd72fcfeebf2fd00c9
```
