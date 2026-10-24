> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Batch planner: exception recovery, performance, and live heap

The framed batch planner recovers from malformed requests and continues to valid work. On the large ThinLTO batches, Imre takes **155–157 ms for layered graphs and about 90 ms for wide graphs**: **2.31–3.94× faster than CPython**, while taking **1.70–1.82× the Rust BigInt reference's time**.

Exception support also has a measurable cost on the unchanged single-request program. Its large workloads take **6.4–9.8% longer with ordinary linking and 8.4–22.7% longer with ThinLTO** than the frozen pre-exception compiler/runtime. These are whole-process compiler/runtime deltas; this experiment does not isolate the cost of an individual exception check or throw.

All timed builds use clean implementation revision `1a18ba752448a800d9d21c479e592ceda513d51f`. The control's prior artifacts come from `347b8d00ac0ce64fc4b4cbbf09e70f2f5f838d2f`. All six original incremental-planner source files and each captured control input are unchanged. The batch program imports that planner's model, scheduling, and rendering code.

## Unchanged-source control

These are separate runs of the original single-request program, not comparisons of a batch with one request. Values are medians over 30 fresh processes. Positive changes mean the new compiler/runtime is slower.

| Mode | Workload | CPython ms | Prior Imre ms | New Imre ms | New/prior change |
|---|---|---:|---:|---:|---:|
| Ordinary | layered 16,384 / sparse | 114.96 | 57.11 | 60.74 | +6.4% |
| Ordinary | layered 16,384 / all | 191.81 | 83.56 | 90.27 | +8.0% |
| Ordinary | wide 16,384 / sparse | 102.77 | 28.45 | 30.95 | +8.8% |
| Ordinary | wide 16,384 / all | 186.04 | 49.99 | 54.89 | +9.8% |
| Ordinary | boundary 128 / all | 20.78 | 3.62 | 3.63 | +0.3% |
| ThinLTO | layered 16,384 / sparse | 114.12 | 50.05 | 54.23 | +8.4% |
| ThinLTO | layered 16,384 / all | 192.33 | 72.63 | 82.72 | +13.9% |
| ThinLTO | wide 16,384 / sparse | 102.49 | 23.19 | 26.88 | +15.9% |
| ThinLTO | wide 16,384 / all | 185.81 | 39.68 | 48.68 | +22.7% |
| ThinLTO | boundary 128 / all | 20.99 | 3.49 | 3.59 | +2.7% |

The 128-target case records only a 0.3% ordinary and 2.7% ThinLTO change in these whole-process measurements. The large cases provide the clearer regression signal.

## Successful and mixed batches

Each valid batch contains two independent copies of the corresponding all-affected planner input. Each mixed batch contains those same two valid requests plus three malformed requests after each: invalid integer text, truncated payload, and a target row missing its duration. Thus a valid batch has two requests and a mixed batch has eight. The malformed requests must emit the specified error, execute cleanup, and leave the following request usable.

Values below are median whole-process milliseconds; brackets show Imre's observed 25th–75th percentiles. CPython/Imre above one favors Imre; Imre/Rust above one favors Rust.

| Mode | Workload | CPython ms | Imre ms [p25–p75] | Rust BigInt ms | CPython/Imre | Imre/Rust |
|---|---|---:|---:|---:|---:|---:|
| Ordinary | layered 16,384 / all / valid | 368.36 | 172.36 [171.17–177.68] | 103.28 | 2.14× | 1.67× |
| Ordinary | layered 16,384 / all / mixed | 366.99 | 171.47 [170.31–174.75] | 102.28 | 2.14× | 1.68× |
| Ordinary | wide 16,384 / all / valid | 359.45 | 103.99 [102.64–105.11] | 55.73 | 3.46× | 1.87× |
| Ordinary | wide 16,384 / all / mixed | 354.57 | 101.74 [101.11–102.34] | 54.77 | 3.49× | 1.86× |
| Ordinary | boundary 128 / all / valid | 22.39 | 4.14 [4.10–4.23] | 3.67 | 5.41× | 1.13× |
| Ordinary | boundary 128 / all / mixed | 22.57 | 4.22 [4.20–4.27] | 3.69 | 5.34× | 1.14× |
| ThinLTO | layered 16,384 / all / valid | 362.74 | 157.16 [155.03–158.39] | 92.02 | 2.31× | 1.71× |
| ThinLTO | layered 16,384 / all / mixed | 361.94 | 155.06 [154.10–156.42] | 91.38 | 2.33× | 1.70× |
| ThinLTO | wide 16,384 / all / valid | 354.28 | 89.96 [89.53–91.10] | 49.31 | 3.94× | 1.82× |
| ThinLTO | wide 16,384 / all / mixed | 354.69 | 89.99 [89.22–91.10] | 49.80 | 3.94× | 1.81× |
| ThinLTO | boundary 128 / all / valid | 22.08 | 3.99 [3.96–4.02] | 3.59 | 5.53× | 1.11× |
| ThinLTO | boundary 128 / all / mixed | 22.20 | 4.08 [4.06–4.12] | 3.61 | 5.44× | 1.13× |

The small differences between valid and mixed medians do not measure a per-exception cost: they include different framing, output, cleanup, and ordinary run-to-run variation. The Rust program implements the same bounded parser and planner algorithm with explicit `Result` recovery and arbitrary-precision integers. It is a direct reference implementation, not a claim about all Rust programs or C.

## Measurement and provenance

The [harness](../batch_benchmark.py) runs two independent three-engine groups: CPython/prior Imre/new Imre for the control, and CPython/new Imre/Rust BigInt for batches. Each workload has two warmup rounds and 30 measured rounds. All six engine permutations occur five times: each engine occupies each position ten times, and each directed adjacent pair within a round occurs ten times. Cross-round predecessor transitions are not separately balanced.

Each sample starts a fresh process pinned with the harness to CPU 0. The timer includes process creation, input delivery, parsing, computation, output transfer, and process exit. It excludes compilation, correctness validation, hashing, and the subsequent comparison of captured output. Every timed and warmup run must have the expected exit status, empty stderr, and byte-for-byte complete stdout. The raw records retain every duration, status, stderr, stdout hash and byte count, plus complete input and expected output once per workload. No samples were discarded. Both modes together contain **1,980 timed runs and 132 warmups**. The harness refuses timing under ptrace.

Ordinary Imre is paired with ordinary Rust; ThinLTO Imre is paired with ThinLTO Rust. ThinLTO Rust uses the recorded explicit import and inline budgets of 1,000, matching the Imre setting. The lexical `ld.lld` path invokes the actual Rust LLD backend, whose hash is recorded; it is not the separate `gcc-ld` wrapper. The timed Imre planner builds explicitly target `x86_64-unknown-linux-gnu`; the Rust references use that same host target.

The host reports an AMD EPYC-Milan processor on Linux x86-64. CPython is 3.14.5, with its GIL enabled, PGO and LTO enabled, and its available JIT disabled. The toolchains are Clang 18.1.3 and Rust 1.98.1-dev (Ohm `ohm-1.98.1-1`, commit `f62703110`), with Rust LLVM/LLD 22.1.8. Different backend versions remain a comparison limitation.

This is a shared devbox, not an isolated machine. Process startup, scheduler interference, allocator behavior, and hash randomization affect these measurements. Validation and warmups precede timing, so this is not a cold-filesystem experiment. All outliers remain in the records: for example, ordinary prior Imre's wide/all control spans 48.21–239.92 ms despite a 49.99 ms median; ordinary Imre's layered/mixed batch spans 166.59–359.12 ms. The ThinLTO wide/valid Imre batch has an 89.53–91.10 ms interquartile range and an 87.91–94.04 ms full range. Percentile spreads are descriptive, not confidence intervals.

The [ordinary raw record](2026-10-18-batch-planner-normal.json), [ThinLTO raw record](2026-10-18-batch-planner-thinlto.json), [frozen compiler/runtime build record](2026-10-18-batch-planner-build.json), and Rust [ordinary](2026-10-18-batch-planner-rust-normal-build.json) / [ThinLTO](2026-10-18-batch-planner-rust-thinlto-build.json) build records retain source, binary, IR, tool, and linked-provenance hashes. The old [ordinary](2026-10-17-incremental-planner-normal.json) and [ThinLTO](2026-10-17-incremental-planner-thinlto.json) records identify the exact reused control binaries. Independent post-run audits checked 131 source/tool/artifact identities, all 2,112 executions, order balance, and recomputed summary statistics.

## Correctness and cleanup

The independent [batch oracle](../batch_validate.py) checks complete output, error categories/messages, continuation, and cleanup across **194 inputs**: **393 completed frames, 342 completed parses, 51 recovered parse failures, and nine terminal framing cases**. Its bounded parser is independently expressed; graph expectations come from the existing independent graph oracle. Valid graph errors remain ordinary planner results. Incomplete outer frames remain terminal. Unicode/NUL names, arbitrary-precision integer boundaries, repeated malformed requests followed by valid work, framing, and error precedence are covered.

Before timing, each batch engine passes all 194 cases. Each control engine passes the original 155 graph cases plus 17 parser cases. Separate validation passed 44 ordinary O0/O3 exception checks and 30 ThinLTO exception checks. Fully instrumented generated code and Rust runtime passed 30 exception checks, 209 container checks, and all 194 batch inputs with ASan/LSan enabled. The batch [sanitizer build record](2026-10-18-batch-planner-sanitizer-build.json) and [validation result](2026-10-18-batch-planner-sanitizer-validation.json) preserve that evidence. The prebuilt Rust standard library was not instrumented. Selected-feature Rust batch Clippy also passed with warnings denied.

Imre's supported exception behavior deliberately omits Python tracebacks, exception context, and chaining. The mixed batches compare the supported observable behavior; they do not equate the implementations' complete exception machinery.

## Live heap during repeated recovery

A separate ordinary-O3 executable exercises eight caught failures per iteration during partially evaluated calls, class construction, list/dictionary/set construction, string concatenation, and a chained comparison. The identical binary receives 100 and 10,000 iterations. These DHAT runs occur after all timings and are not performance samples.

| Iterations | Caught failures | Total allocated bytes | Allocation blocks | Peak live requested bytes | Peak live blocks | End-live bytes / blocks |
|---:|---:|---:|---:|---:|---:|---:|
| 100 | 800 | 346,960 | 6,408 | 18,140 | 14 | 17,408 / 3 |
| 10,000 | 80,000 | 32,957,560 | 640,008 | 18,140 | 14 | 17,408 / 3 |

Allocation traffic grows with repetitions while peak and end-live requested heap remain unchanged. The three end-live allocations are persistent I/O buffers: an 8,192-byte Imre output buffer, an 8,192-byte Rust stdin buffer, and a 1,024-byte Rust stdout buffer. None is exception-owned. This supports bounded cleanup for these exercised failure paths; it is not a proof for every program. **DHAT measures requested heap, not RSS or allocator-reserved memory.**

The [memory evidence](2026-10-18-batch-planner-recovery-memory.json) contains the exact stress source, its derivation from the exception fixture, input and complete output, artifact/tool hashes, and both complete normalized DHAT traces. Normalization changes private paths only; numeric profiling data is retained. Peak/end-live sums can be recomputed from the traces' `pps` entries.

## Compilation and artifact sizes

These are one observed source-to-executable compilation per program, including linking. Existing compiler/runtime and Rust reference builds are excluded from execution timing; these are not cold toolchain-build measurements.

| Mode | Control compile s | Batch compile s | Control executable bytes | Batch executable bytes | Rust batch bytes |
|---|---:|---:|---:|---:|---:|
| Ordinary | 0.670 | 0.768 | 3,151,760 | 3,155,912 | 1,504,072 |
| ThinLTO | 4.665 | 5.044 | 2,799,912 | 2,891,256 | 1,720,848 |

The generated LLVM IR is 187,589 bytes for the control and 273,928 bytes for the batch program in either mode.

## Reproduction

The [example README](../../examples/batch_planner/README.md) describes the framing contract, oracle commands, and ordinary/ThinLTO reference builds. Use the repository's pinned normal Cargo toolchain for public reproduction; local Ohm builds use the explicitly recorded configuration.

The control requires a preserved pre-exception binary and its matching incremental benchmark record. Reproduce that baseline at the prior revision using the incremental harness, then preserve both before building the new revision. A newly rebuilt binary cannot be paired with a published record whose binary hash differs. The following uses the artifact names from this run and a local CPython 3.14 executable:

```sh
python3 benchmarks/batch_benchmark.py \
  --compiler benchmarks/local/batch-tools-final/imre \
  --runtime benchmarks/local/batch-tools-final/libimre_runtime.a \
  --imre-build-record benchmarks/local/batch-tools-final/build-record.json \
  --baseline benchmarks/local/incremental-artifacts-normal-final/incremental_planner \
  --baseline-record benchmarks/results/2026-10-17-incremental-planner-normal.json \
  --rust-bigint benchmarks/local/batch-reference-normal-final/planner-batch-bigint \
  --rust-build-record benchmarks/local/batch-reference-normal-final/build-record.json \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --artifact-dir benchmarks/local/batch-normal-reproduction \
  --output benchmarks/local/batch-normal-reproduction.json \
  --samples 30 --warmups 2 --cpu 0
```

For ThinLTO, use the bitcode runtime archive, the preserved ThinLTO baseline and record, the Rust ThinLTO reference and build record, and distinct output paths. Add `--lto-linker PATH/ld.lld` pointing through a lexical `ld.lld` symlink to the actual Rust LLD backend. Build the Rust reference with `CARGO_PROFILE_RELEASE_LTO=thin` and explicit `--llvm-budget 1000`. The harness verifies mode, target, source, and artifact identities before measuring.

The memory experiment is independently reproducible from the public record without a private helper. Extract its exact source and inputs:

```sh
python3 - <<'PYTHON'
import json
from pathlib import Path
record = json.loads(Path(
    "benchmarks/results/2026-10-18-batch-planner-recovery-memory.json"
).read_text())
out = Path("benchmarks/local/recovery-reproduction")
out.mkdir(parents=True, exist_ok=True)
(out / "stress.py").write_text(record["source"])
for run in record["runs"]:
    stem = str(run["iterations"])
    (out / (stem + ".stdin")).write_text(run["input"])
    (out / (stem + ".expected")).write_text(run["expected_stdout"])
PYTHON

benchmarks/local/batch-tools-final/imre build \
  benchmarks/local/recovery-reproduction/stress.py \
  --runtime benchmarks/local/batch-tools-final/libimre_runtime.a \
  --clang clang-18 --opt-level 3 \
  --emit-llvm benchmarks/local/recovery-reproduction/stress.ll \
  -o benchmarks/local/recovery-reproduction/stress

for count in 100 10000; do
  valgrind --tool=dhat --mode=heap \
    --dhat-out-file=benchmarks/local/recovery-reproduction/$count.dhat.json \
    --log-file=benchmarks/local/recovery-reproduction/$count.log \
    benchmarks/local/recovery-reproduction/stress \
    < benchmarks/local/recovery-reproduction/$count.stdin \
    > benchmarks/local/recovery-reproduction/$count.stdout
  cmp benchmarks/local/recovery-reproduction/$count.expected \
      benchmarks/local/recovery-reproduction/$count.stdout
done
```

The recorded run used Valgrind 3.22.0. Use the same executable for both iteration counts; compare `tb`, `tbk`, `gb`, `gbk`, `eb`, and `ebk` summed across each trace's `pps` entries.
