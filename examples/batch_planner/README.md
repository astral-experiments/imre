# Framed batch dependency planner

This package runs independent named incremental rebuild requests in one process.
It imports the existing incremental planner's `Task`, scheduler, and renderer
unchanged. A bounded payload parser demonstrates useful exception recovery:
malformed requests produce a deterministic report, release their buffered input,
and allow later requests to run.

Input starts with a nonnegative request count. Each request starts with a
nonnegative line count, followed by exactly that many payload lines. The payload
uses the [incremental planner's input format](../incremental_planner/README.md):
target count, target rows, change count, and changed names. Counts and durations
use the supported ASCII decimal integer syntax, with signs and digit separators.
Names may contain Unicode or NUL but no whitespace.

The outer reader collects an entire frame before parsing it. An invalid outer
count or premature outer EOF terminates the program; an incomplete frame never
prints a `REQUEST` header. A payload cannot consume lines from the next request.
Extra payload lines are ignored, preserving the single-request parser's behavior,
and are still released during cleanup. Extra input after the final frame is ignored.

For each complete frame, output starts with `REQUEST` and its zero-based ID on
separate lines. A successfully parsed payload produces the unchanged incremental
report. Its graph validation phases remain duplicate names, negative durations,
unknown dependencies, unknown changed names, and whole-graph cycles, in that order.
Graph `ERROR` results are data and count as completed requests. No-change requests
still validate the entire graph; unaffected prerequisites count as already built.

A payload parsing error instead prints `INVALID`, its exception category, and a
stable message, each on a separate line:

| Category | Message |
| --- | --- |
| `ValueError` | `invalid integer`, `negative target count`, or `negative change count` |
| `IndexError` | `truncated payload` |
| `AssertionError` | `target row needs a name and duration` or `change row needs exactly one name` |

The `finally` block empties the caller-visible input buffer and prints `END` and
the request ID. The caller asserts that cleanup emptied the buffer. After all
frames, `SUMMARY`, `COMPLETED`, its count, `INVALID`, its count, `CLEANED`, and its
count are printed on separate lines. A terminal framing failure preserves earlier
request output but does not print a final summary.

For example, this batch contains a malformed duration followed by a valid request:

```text
2
3
1
bad x
0
4
1
app 7
1
app
```

Run the unchanged package on CPython, or compile it with Relar:

```sh
PYTHONPATH=examples python3.14 -m batch_planner < batch-input.txt
target/release/relar build examples/batch_planner/__main__.py \
  --module-root examples --runtime target/release/librelar_runtime.a \
  --clang clang-18 --opt-level 3 -o benchmarks/local/batch_planner
benchmarks/local/batch_planner < batch-input.txt
```

The [Rust reference](../../benchmarks/reference/incremental_planner/README.md)
shares its existing graph algorithm and adds a bounded batch parser with explicit
`Result` recovery. Build its arbitrary-precision entrypoint with normal Cargo in
public/CI environments:

```sh
python3 benchmarks/reference/incremental_planner/build.py --batch \
  --artifact-dir benchmarks/local/batch-reference-normal \
  --target-dir benchmarks/local/batch-reference-normal/target
python3 benchmarks/batch_validate.py --python python3.14 \
  --binary benchmarks/local/batch_planner \
  --rust-bigint benchmarks/local/batch-reference-normal/planner-batch-bigint
```

On this Ohm development environment, add `--toolchain ohm` to the build helper;
it selects `cargo +ohm -Zohm-defaults=no` and records the exact compiler, flags,
source text/hashes, helper, and resulting binary. Use a distinct target directory
and `--build-cache` for a shared cache when appropriate. The default single-request
reference and its two numeric modes remain unchanged. Batch comparisons use BigInt;
they make no checked-i64 equivalence claim.

The independent oracle checks 194 complete inputs: all 155 graph cases alone and
together, the 17 existing parser cases followed by recovery, repeated failures,
failure after partial task construction, huge bounded payload counts, Unicode/NUL
names, CRLF, missing final newline, ignored trailing input, and nine terminal
framing cases. It uses an iterator-based payload grammar and the existing
`graphlib`/fixed-point/`heapq` oracle, never the compiled package's parser or scheduler.
Every engine must match complete stdout and exit status; terminal errors also match
their final error category. Successful processes must emit no stderr.

The benchmark has two deliberately separate groups:

- **Unchanged-program control:** frozen pre-exception Relar, newly compiled Relar,
  and CPython execute identical original incremental source and captured inputs.
  This measures compiler/runtime regression on code with no exception handlers.
- **Batch program:** Relar, CPython, and Rust BigInt execute valid and mixed batches.
  A mixed batch adds three recoverable malformed frames after each valid frame.
  This measures framing, parsing, errors, cleanup, graph work, and reporting together.

```sh
python3 benchmarks/batch_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --baseline benchmarks/local/incremental-artifacts-normal-final/incremental_planner \
  --baseline-record benchmarks/results/2026-10-17-incremental-planner-normal.json \
  --rust-bigint benchmarks/local/batch-reference-normal/planner-batch-bigint \
  --rust-build-record benchmarks/local/batch-reference-normal/build-record.json \
  --python python3.14 --clang clang-18 \
  --artifact-dir benchmarks/local/batch-normal \
  --output benchmarks/local/batch-normal.json
```

Use the actual saved baseline record path if it differs. The harness verifies its
exact binary and all six original Python source hashes, reconstructs every captured
input through the independent oracle, and requires matching ordinary/ThinLTO mode
and target. It compiles each new Relar program once outside timed work, then runs
the full original and batch correctness suites before measuring any workload.
`--relar-build-record` additionally verifies frozen compiler/runtime and compiler
source identities. `--rust-source-root` supports saved sources with their original
build record; those Rust sources must match the tracked reference exactly.

Defaults are all five captured single-request controls and valid/mixed batches
from the three `all`-changes workloads, with two valid payloads per batch. Select
captured labels using repeated `--control-workload` and `--batch-workload`, and
change batch repetition with `--requests`. For a small harness check, select
`boundary-128-all` for both groups, with `--requests 1 --samples 6 --warmups 1`.

Each workload has two warmups and 30 measured rounds by default. All six engine
orders repeat, balancing positions and directed within-round predecessors;
cross-round predecessors are not balanced. Every sample is a fresh process pinned
to one allowed CPU, and ptrace timing is refused. Time includes process startup,
CPython imports, pipes, complete computation/output, and exit. Builds, oracle work,
warmups, comparisons, and hashes are excluded. Full inputs and expected stdout are
stored once per workload; every raw run retains duration, status, stderr, output
hash and byte count after exact comparison. Records retain source/tool/artifact
hashes, compiler and CPython configuration, CPU/OS/affinity, medians, and spread.

For ThinLTO, build a separate batch reference with
`CARGO_PROFILE_RELEASE_LTO=thin` and `--llvm-budget 1000`, select the bitcode Relar
runtime and matching prior ThinLTO baseline/record, and pass
`--lto-linker /path/to/ld.lld --target x86_64-unknown-linux-gnu`. Use distinct artifact
and JSON paths. The lexical linker name is preserved for LLD flavor while its
resolved contents are hashed. Ordinary and ThinLTO comparisons remain separate.

Batch-versus-single timings are not an isolated measure of exception overhead:
the new program adds buffering, framing, cleanup, and output. The unchanged-source
control addresses regression separately. Shared-machine variation, allocation,
process startup, and hash seeding remain; these are workload-specific results.
Rust uses the same graph algorithm with native indices/containers and explicit
error results. Its timings do not establish general Rust, C, or Python speed.
