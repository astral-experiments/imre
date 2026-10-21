> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Concrete layouts and unboxed optionals

Concrete class/tuple layouts, nullable-word optionals, and specialized container storage reduce elapsed time on every large planner workload. With matched ThinLTO, the unchanged programs take **11.5–16.4% less time** than the previous compiler/runtime. They run **2.20–5.24× faster than CPython**, while taking **1.32–1.55× Rust BigInt's time**. Rust remains faster. Ordinary builds improve by **8.4–15.4%** on the same large inputs.

The implementation is commit `6d0db50ab7851f4321704e6f97b79a2d92d58606`; the final benchmark harness and clean measured checkout are `4f60c860364e6f0d2cfbf9cc3a5b656f56988216`. The preserved baseline is `32dd63afa5599cb8d61d49e2730b3f6eeae87755`. Application sources, captured inputs, and Rust reference algorithms are unchanged. The tool-build snapshot predates the harness commit and records then-untracked benchmark files; every compiled source hash matches both the implementation and final timing commits.

## What changed

The compiler retains exact field types through IR and emits concrete LLVM layouts for classes and escaping fixed tuples. A record has a 16-byte reference-count/descriptor header and one eight-byte word per field on the supported 64-bit target. Field access uses typed loads and stores instead of a generic object accessor. Construction zeroes owning fields so partial initialization can be cleaned up after constructor failure. Replacement retains the new value before releasing the old value, preserving aliases.

Optional integers and pointer values use zero for absence and their existing nonzero tagged/pointer word for presence. Optional booleans encode `0=None`, `1=False`, and `2=True`. Integer zero remains distinct from `None`; arbitrary-precision integers still use their heap representation when necessary. `dict.get`/lookup keeps a separate found bit, so a present zero, `False`, or `None` is never confused with a missing key. Present values no longer require an optional wrapper allocation.

Generic sequences store raw words with one element kind per container. Dictionaries specialize integer/string keys and keep one value kind; sets have no dummy value. This removes repeated per-entry value tags while retaining dictionary insertion order, iterator mutation checks, and ownership. The runtime still uses reference counting over statically acyclic heap types. This change neither adds a tracing GC nor relaxes the ownership contract. The private native ABI changed: generated code must be rebuilt and linked with its matching runtime.

## Measurement method

The four engines are CPython 3.14.5, preserved Relar, new Relar, and the preserved Rust BigInt reference. Each workload has 32 measured rounds and two warmups per engine. Four Williams orders repeat eight times, balancing positions and directed adjacent pairs within each measured round cycle. Cross-round predecessors are not balanced. Every sample starts a fresh process pinned to CPU 0. Ordinary and ThinLTO runs were sequential, outside ptrace, after all task builds/tests/profilers stopped. No samples were discarded.

Timing includes process startup, imports, parsing, validation, scheduling, complete output, and exit. Batch cases include framing, recovery, and cleanup. Manifest cases also include file opens, reads, writes, flushes, and closes. Fixture creation, oracle execution, output/filesystem comparisons, hashing, and deletion are outside the timer. Manifest inputs are page-cache warm; output is not fsynced. These are whole-program measurements on a shared devbox, with uncontrolled frequency and shared-resource noise, rather than isolated layout costs or cold-storage throughput.

There are five incremental workloads and six each for batch and manifest, for **4,352 measured executions and 272 warmups** across both modes. All pass complete stdout, stderr, exit-status, and applicable filesystem comparisons. Before timing, every engine also passes the full independent application oracles. The final raw records retain all inputs, expected outputs, samples, ordering, environment checks, source hashes, build records, and executable identities. An independent audit recomputed every median, quartile, and pairwise ratio from integer nanoseconds, checked all 4,504 saved oracle/parser/device results, and verified 112 distinct files. It confirms every planned sample is present; it does not claim an independent operating-system process log.

## Whole-program results

Times are median milliseconds. Brackets show observed interquartile ranges, not confidence intervals. All layered/wide inputs contain 16,384 targets; boundary inputs contain 128 and exercise arbitrary-precision values. Valid batches/manifests contain two valid all-changed requests; mixed variants add six recovered malformed/missing-input requests. `Change` is `new / previous − 1`; negative is faster. `CPython/Relar` is a speed ratio; `Relar/Rust` above one means Rust is faster.

### Ordinary linking

| Program / input | CPython | Previous Relar | New Relar [Q1–Q3] | Rust BigInt | Change | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| incremental / layered-sparse | 115.11 | 59.53 | 50.39 [49.95–50.77] | 35.01 | -15.4% | 2.28× | 1.44× |
| incremental / layered-all | 193.05 | 88.65 | 75.11 [74.54–75.71] | 54.04 | -15.3% | 2.57× | 1.39× |
| incremental / wide-sparse | 101.20 | 28.73 | 26.31 [26.01–26.76] | 16.51 | -8.4% | 3.85× | 1.59× |
| incremental / wide-all | 185.28 | 51.00 | 44.49 [44.04–44.96] | 28.92 | -12.8% | 4.16× | 1.54× |
| incremental / boundary-128-all | 21.37 | 3.73 | 3.75 [3.67–3.89] | 3.55 | +0.5% | 5.70× | 1.06× |
| batch / layered-all-valid | 364.25 | 163.60 | 144.23 [142.79–145.79] | 102.00 | -11.8% | 2.53× | 1.41× |
| batch / layered-all-mixed | 362.76 | 161.94 | 143.48 [141.04–145.32] | 101.66 | -11.4% | 2.53× | 1.41× |
| batch / wide-all-valid | 350.89 | 94.05 | 85.24 [84.01–86.15] | 55.43 | -9.4% | 4.12× | 1.54× |
| batch / wide-all-mixed | 354.69 | 94.25 | 85.47 [85.08–86.90] | 55.69 | -9.3% | 4.15× | 1.53× |
| batch / boundary-128-all-valid | 22.30 | 4.09 | 4.05 [4.01–4.08] | 3.67 | -0.9% | 5.50× | 1.11× |
| batch / boundary-128-all-mixed | 23.15 | 4.24 | 4.21 [4.16–4.29] | 3.71 | -0.7% | 5.50× | 1.13× |
| manifest / layered-all-valid | 262.46 | 162.89 | 142.81 [141.38–144.19] | 89.58 | -12.3% | 1.84× | 1.59× |
| manifest / layered-all-mixed | 260.63 | 163.70 | 142.50 [141.38–145.98] | 89.86 | -13.0% | 1.83× | 1.59× |
| manifest / wide-all-valid | 251.23 | 97.45 | 86.36 [85.56–87.42] | 47.57 | -11.4% | 2.91× | 1.82× |
| manifest / wide-all-mixed | 256.35 | 98.00 | 87.60 [86.87–89.03] | 48.28 | -10.6% | 2.93× | 1.81× |
| manifest / boundary-128-all-valid | 29.83 | 3.42 | 3.34 [3.21–4.29] | 2.80 | -2.4% | 8.92× | 1.19× |
| manifest / boundary-128-all-mixed | 30.35 | 4.68 | 4.65 [3.77–4.76] | 2.98 | -0.7% | 6.53× | 1.56× |

### ThinLTO

| Program / input | CPython | Previous Relar | New Relar [Q1–Q3] | Rust BigInt | Change | CPython/Relar | Relar/Rust |
|---|---:|---:|---:|---:|---:|---:|---:|
| incremental / layered-sparse | 113.77 | 51.92 | 43.73 [43.21–44.51] | 31.65 | -15.8% | 2.60× | 1.38× |
| incremental / layered-all | 194.65 | 78.32 | 65.95 [64.47–67.00] | 49.88 | -15.8% | 2.95× | 1.32× |
| incremental / wide-sparse | 103.68 | 24.39 | 21.57 [21.45–22.02] | 15.61 | -11.5% | 4.81× | 1.38× |
| incremental / wide-all | 187.59 | 42.41 | 36.14 [35.44–36.66] | 26.74 | -14.8% | 5.19× | 1.35× |
| incremental / boundary-128-all | 21.10 | 3.54 | 3.55 [3.49–3.59] | 3.46 | +0.4% | 5.94× | 1.03× |
| batch / layered-all-valid | 363.40 | 145.02 | 123.48 [122.30–125.25] | 92.55 | -14.9% | 2.94× | 1.33× |
| batch / layered-all-mixed | 364.36 | 144.66 | 122.83 [122.06–124.80] | 91.93 | -15.1% | 2.97× | 1.34× |
| batch / wide-all-valid | 353.30 | 77.76 | 67.70 [67.18–68.44] | 49.48 | -12.9% | 5.22× | 1.37× |
| batch / wide-all-mixed | 353.24 | 77.26 | 67.42 [67.00–68.12] | 49.25 | -12.7% | 5.24× | 1.37× |
| batch / boundary-128-all-valid | 22.30 | 3.94 | 3.87 [3.83–3.92] | 3.60 | -1.8% | 5.76× | 1.08× |
| batch / boundary-128-all-mixed | 22.46 | 4.06 | 4.00 [3.96–4.04] | 3.65 | -1.6% | 5.62× | 1.10× |
| manifest / layered-all-valid | 263.62 | 143.05 | 119.64 [118.53–120.96] | 82.52 | -16.4% | 2.20× | 1.45× |
| manifest / layered-all-mixed | 264.64 | 142.01 | 119.32 [118.78–120.95] | 82.04 | -16.0% | 2.22× | 1.45× |
| manifest / wide-all-valid | 253.41 | 77.96 | 67.60 [67.12–68.76] | 43.48 | -13.3% | 3.75× | 1.55× |
| manifest / wide-all-mixed | 255.04 | 78.42 | 68.55 [67.91–70.24] | 44.16 | -12.6% | 3.72× | 1.55× |
| manifest / boundary-128-all-valid | 29.62 | 3.31 | 3.13 [3.08–3.40] | 2.84 | -5.4% | 9.46× | 1.10× |
| manifest / boundary-128-all-mixed | 29.66 | 3.60 | 3.43 [3.35–4.69] | 3.02 | -4.5% | 8.63× | 1.14× |

The small incremental boundary case moves by only +0.5% ordinary and +0.4% with ThinLTO, with overlapping interquartile ranges. Small batch changes also overlap observed spread. The boundary manifest runs are noisy, especially mixed requests; their median reductions are not evidence of a general startup improvement. Large valid/mixed comparisons do not isolate an individual exception's cost. No C comparison was performed.

## Allocation and instruction evidence

Executed instructions for the 16,384-target incremental planner:

| Mode / input | Previous Relar | New Relar | Change | Rust BigInt |
|---|---:|---:|---:|---:|
| Ordinary / layered/all | 487,630,924 | 454,962,733 | -6.7% | 271,386,648 |
| Ordinary / wide/all | 314,042,002 | 308,442,924 | -1.8% | 164,852,929 |
| ThinLTO / layered/all | 415,032,715 | 376,646,078 | -9.2% | 227,404,996 |
| ThinLTO / wide/all | 261,665,085 | 251,013,736 | -4.1% | 136,997,037 |

Allocation totals are identical between ordinary and ThinLTO profiles:

| Input / engine | Allocations | Requested bytes | Peak live requested bytes |
|---|---:|---:|---:|
| layered/all / previous Relar | 463,279 | 41,672,660 | 14,971,708 |
| layered/all / new Relar | 407,993 | 25,974,524 | 11,197,596 |
| layered/all / Rust BigInt | 310,488 | 19,556,738 | 9,003,632 |
| wide/all / previous Relar | 295,190 | 28,740,141 | 11,520,808 |
| wide/all / new Relar | 278,805 | 19,696,173 | 8,243,584 |
| wide/all / Rust BigInt | 196,821 | 13,266,251 | 6,145,620 |

On layered/all, allocation count falls 11.9%, requested bytes 37.7%, and peak live requested heap 25.2%. Wide/all reduces these by 5.6%, 31.5%, and 28.4%. These profiles still show a substantial instruction and allocation gap to Rust.

The storage probe retains and reads 10,000 present integer optionals and 10,000 present object optionals. Allocation count falls from 30,035 to 10,035: exactly **20,000 wrapper allocations disappear**. Requested bytes fall from 3,706,128 to 781,856 and peak live requested heap from 3,181,872 to 519,680. A one-field record uses 24 bytes, a four-field record 48 bytes, and a two-field tuple 32 bytes, versus 88 bytes for the former generic record allocation. String constants are shared so this experiment measures storage rather than repeated string construction. It is a mechanism probe, not an application speed benchmark.

Callgrind counts executed instructions, not CPU cycles or elapsed-time percentages. DHAT reports requested allocation traffic and live heap, not RSS. Hardware performance counters were unavailable on this host. Profiles are separate from wall timing, and the evidence identifies the exact executable behind each trace. They support the reduced allocation/storage mechanism but do not divide the elapsed-time gain among individual implementation changes.

## Validation and toolchain

The final compiler/runtime pass 19 compiler and 36 runtime unit tests, formatting, and Clippy. Native differential validation covers **1,609 checks**: 641 ordinary O0/O3 checks, 479 ThinLTO O3 checks, 479 AddressSanitizer O1 checks, and 10 ThinLTO CLI checks. The seven semantic suites cover integers, imports, classes, containers, exceptions, interactive/output I/O, and files/arguments. Sanitizer runs instrument generated code and the Rust runtime/target dependencies, with leak detection enabled outside ptrace; the prebuilt Rust standard library is not instrumented. Compiler unit-test success was captured in the task output without a standalone local log; the published validation record marks that limitation and also records the successful [implementation CI run](https://github.com/viarius-experiments/relar/actions/runs/39127175500).

The application gates cover 155 graph and 17 parser cases, 194 framed-batch cases, and 196 manifest cases plus a separate Linux `/dev/full` check, for all four engines in both linking modes. These include full report bytes, malformed inputs, recovery, Unicode, aliases, and arbitrary-precision boundaries. Independent implementation review also checked mixed pointer/integer record loads so LLVM pointer fields retain valid Rust pointer provenance during destruction.

The host is Linux x86-64 on AMD EPYC-Milan. CPython 3.14.5 uses the GIL, has JIT disabled, and was built with PGO/LTO. Relar uses Clang 18.1.3 and Rust 1.98.1-dev (`f62703110`, Ohm `ohm-1.98.1-1`, LLVM 22.1.8). ThinLTO links with LLD 22.1.8 and uses import/inline budgets of 1000 for both native implementations. The frontend/runtime implementations and backend versions still differ. Per-application AOT compilation takes 0.687–0.783 seconds ordinary and 5.580–6.658 seconds with ThinLTO, excluding compiler/runtime builds; compilation is outside execution timings.

## Evidence and reproduction

- [Ordinary raw samples](2026-10-18-concrete-layouts-normal.json.gz) and [ThinLTO raw samples](2026-10-18-concrete-layouts-lto.json.gz).
- [Independent sample audit and script](2026-10-18-concrete-layouts-audit.json), [compiler/runtime build and exact source record](2026-10-18-concrete-layouts-build.json), and [native validation with CI result](2026-10-18-concrete-layouts-validation.json).
- [Profile summary and executable identity bridge](2026-10-18-concrete-layouts-profiles.json), [raw trace/source/build evidence archive](2026-10-18-concrete-layouts-evidence.tar.gz), and [archive member/hash manifest](2026-10-18-concrete-layouts-evidence-manifest.json).

The [comparison guide](../LAYOUTS.md) describes the preserved baseline and gives validation, timing, and profiling commands; the evidence archive contains exact source snapshots and build records. Build the recorded source with the pinned toolchain or recorded local Ohm wrapper, retain matching compiler/runtime provenance, and use a separately sealed baseline index. The harness refuses source, executable, target, mode, and reference-build mismatches. It preserves the original Rust executables and source/build snapshots rather than relabeling rebuilt controls.

The timing gzip files preserve their original JSON bytes with `mtime=0`. Decompressed SHA256 values are `e826aee29761714adaaed22dcc9ff8f6eab72b831c40cc924d54f3a4e8b3d702` (ordinary) and `90a2c10ef9b60c612903f342de1bdbfbbbdbf91c001a43ac2552f8d5fd76726f` (ThinLTO). Local originals are retained.
