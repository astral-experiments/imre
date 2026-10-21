# Incremental planner reference and comparison

This Rust package directly implements the algorithm in
[the unchanged Python package](../../../examples/incremental_planner/README.md):
name resolution, all five validation phases, per-target dependency deduplication,
whole-graph heap validation, set-and-queue affected closure, a second handwritten
min-heap schedule, and the complete report. It does not replace the heap with a
different scheduler or omit validation from the measured work.

Both Rust modes use the same source. BigInt uses pinned `num-bigint = 0.4.8` and
`num-traits = 0.2.19` for durations and schedule times. Checked-i64 uses signed
64-bit values and explicitly rejects out-of-range input or accumulated finishes;
it never wraps. IDs, counts, and heap positions use native `usize`. Strings and
containers use ordinary Rust `String`, `Vec`, `HashMap`, and `HashSet`; map keys
borrow stored task names. Python's dictionary insertion-order duration check is
an equivalent input-order task walk in Rust. Set order does not affect output.

The reference accepts the example's ASCII decimal syntax, including optional
signs and underscores between digits. Whitespace splitting includes U+001C–1F.
Names may contain Unicode or NUL but cannot contain whitespace. Output uses an
8 KiB pipe/file buffer, newline flushing on terminals, and an exit flush.
Traceback text, invalid UTF-8, signals, recoverable allocation failure, and
unaddressable input counts are outside its compatibility contract.

Build ordinary references with normal Cargo in public/CI environments:

```sh
python3 benchmarks/reference/incremental_planner/build.py \
  --artifact-dir benchmarks/local/incremental-reference-normal \
  --target-dir benchmarks/local/incremental-reference-normal/target
```

On an Ohm development machine, add `--toolchain ohm`; the helper runs
`cargo +ohm -Zohm-defaults=no` and `rustc +ohm`. `--build-cache` selects a shared
cache distinct from the target directory. Build records preserve source text
and hashes, the exact helper, binary/tool hashes, commands, versions, flags, and
build durations. The lockfile is never regenerated. Inherited Rust target flags
are cleared. Normal release builds use generic CPU settings and no explicit LTO
or LLVM budget overrides.

Build separate matched ThinLTO references:

```sh
CARGO_PROFILE_RELEASE_LTO=thin \
  python3 benchmarks/reference/incremental_planner/build.py --llvm-budget 1000 \
  --artifact-dir benchmarks/local/incremental-reference-lto \
  --target-dir benchmarks/local/incremental-reference-lto/target
```

The explicit budget sets LLVM's import and inlining limits to 1,000, matching
Relar's optional O3 ThinLTO mode. Other `CARGO_PROFILE_RELEASE_*` overrides are
recorded; the comparison requires release optimization level 3 and matching
LTO/budget settings. Build the Relar runtime normally for the ordinary comparison,
or with `RUSTFLAGS=-Clinker-plugin-lto` in a separate target directory for ThinLTO,
as described in [the subset guide](../../../docs/subset.md).

Correctness runs independently of benchmarks:

```sh
python3 benchmarks/incremental_validate.py \
  --binary benchmarks/local/incremental_planner \
  --rust-bigint benchmarks/local/incremental-reference-normal/planner-bigint \
  --rust-i64 benchmarks/local/incremental-reference-normal/planner-i64 \
  --python python3.14
```

The oracle uses `graphlib` topology, a fixed-point affected-set computation, and
independent dependency maximums. It never imports the package or uses its
successor lists, indegrees, handwritten heap, or closure queue. It checks full
output for valid and invalid graphs, none/partial/all changes, duplicate names,
dependencies and changes, disconnected cycles, error precedence, huge integers,
Unicode/NUL names, CRLF, and numeric boundaries. Checked-i64 exclusions are
recorded and additionally tested for explicit `OverflowError` rejection.
The 155 graph cases are accompanied by 17 separately recorded parser checks:
14 malformed inputs require the same exit status, empty stdout, and final error
category; three valid framing variants require the complete oracle output.
These cover missing lines/tokens, integer syntax, parsing before graph validation,
ignored trailing input, a missing final newline, and the different whitespace
rules of `int()` and `split()`.

An ordinary four-engine comparison:

```sh
python3 benchmarks/incremental_benchmark.py \
  --compiler target/release/relar --runtime target/release/librelar_runtime.a \
  --rust-bigint benchmarks/local/incremental-reference-normal/planner-bigint \
  --rust-i64 benchmarks/local/incremental-reference-normal/planner-i64 \
  --rust-build-record benchmarks/local/incremental-reference-normal/build-record.json \
  --python python3.14 --clang clang-18 \
  --artifact-dir benchmarks/local/incremental-normal \
  --output benchmarks/local/incremental-normal.json
```

For ThinLTO, select the ThinLTO runtime archive and Rust artifacts/record, add
`--lto-linker /path/to/ld.lld --target x86_64-unknown-linux-gnu`, and use different
artifact/output paths. The lexical `ld.lld` name is preserved for linker flavor;
its resolved file contents are hashed. An optional `--relar-build-record` checks
frozen compiler/runtime identities and source hashes. Existing Rust artifacts
can be reused with their original record and `--rust-source-root`; their Cargo
and Rust sources must still match this tracked reference exactly.

Default workloads are 16,384-target layered and wide graphs with sparse and all
changes, plus a 128-target independent graph with durations around the tagged
integer and signed-64-bit boundaries and multibyte names. Repeat `--workload
SHAPE:SIZE:CHANGES` to select other cases: shapes are `layered`, `wide`, and
`boundary`; change selections are `sparse`, `all`, and `none`. `--seed` defaults
to 20261017, so ordinary and ThinLTO runs generate identical inputs. For example,
`--workload wide:128:all --samples 4 --warmups 1` is a small harness check.

The harness compiles Relar once at O3, outside workload timing, then validates all
engines against the oracle. By default each workload has two warmups and 32
measured rounds. Four Williams orders balance every execution position and every
directed adjacent pair within a round; cross-round predecessors are not balanced.
Each run starts a fresh process. The harness and children share one allowed CPU
(the first allowed CPU by default, or `--cpu`). Traced execution is refused.

Time includes process startup, input/output pipes, parsing, whole-graph validation,
closure, scheduling, reporting, and exit; CPython imports the unchanged package.
Builds, correctness checks, workload generation, warmups, output comparisons,
and hashes are excluded. Every run must exit successfully, emit no stderr, and
match the entire expected stdout. Each workload retains its full input and
expected output once; raw runs retain time, status, stderr, output hash and length.
Records also preserve source/tool/artifact hashes, CPU/OS/affinity, CPython
JIT/GIL/PGO/LTO information, every execution order, medians, and observed spread.

These are whole-program results on a shared devbox, including variable startup,
allocator, scheduling, and hash-seed effects. Pinning and counterbalancing reduce
some variation but do not isolate kernels or establish general language speed.
Rust ports the algorithm in different source with native indices and different
representations; checked-i64 has a narrower numerical domain. Compare ordinary
and ThinLTO records separately, and retain each run's raw spread.

## Batch entrypoint

The optional `batch` feature adds a separate `planner-batch` executable for the
[framed batch example](../../../examples/batch_planner/README.md). It reuses the
same `Task`, scheduler, and renderer, and adds bounded payload parsing, explicit
`Result` recovery, buffer cleanup, and deterministic request/summary output.
The default `planner-reference` behavior is unchanged.

```sh
python3 benchmarks/reference/incremental_planner/build.py --batch \
  --artifact-dir benchmarks/local/batch-reference-normal \
  --target-dir benchmarks/local/batch-reference-normal/target
python3 benchmarks/batch_validate.py --python python3.14 \
  --rust-bigint benchmarks/local/batch-reference-normal/planner-batch-bigint
```

`--batch` builds the arbitrary-precision binary only and records `entrypoint:
batch`; the default artifact/target locations are separate from single-request
reference builds. It supports the same toolchain/cache and explicit LLVM-budget
options. A matched ThinLTO build uses `CARGO_PROFILE_RELEASE_LTO=thin` and
`--llvm-budget 1000` with distinct artifact/target directories. Batch measurements
make no checked-i64 equivalence claim. Unaddressable outer framing counts remain
outside the reference contract; huge counts inside a bounded payload preserve
earlier parse errors and eventually report truncation, matching Python.

The [batch benchmark](../../batch_benchmark.py) separates a regression control
using identical old single-request source/input from comparisons of the new
valid/mixed batch program. Its three-engine groups use all six execution orders
over 30 rounds, with full-output checks and source/build provenance. Framing and
cleanup make batch-versus-single time unsuitable as isolated exception overhead.

## Manifest entrypoint

The optional `manifest` feature adds `planner-manifest` for the
[file-backed manifest example](../../../examples/manifest_planner/README.md).
It reuses the same bounded parser, graph validation, scheduler, and renderer.
It reads and closes the complete manifest before opening the report, then reads
each exact payload path relative to the process working directory. Strict UTF-8,
Python's full `splitlines()` boundary set, normalized input failures, request
headers, and complete report bytes follow the example's contract. Report writes
remain outside input recovery.

```sh
python3 benchmarks/reference/incremental_planner/build.py --manifest \
  --artifact-dir benchmarks/local/manifest-reference-normal \
  --target-dir benchmarks/local/manifest-reference-normal/target
python3 benchmarks/manifest_validate.py --python python3.14 \
  --rust-bigint benchmarks/local/manifest-reference-normal/planner-manifest-bigint
```

`--manifest` builds only the arbitrary-precision executable and records
`entrypoint: manifest`; it is mutually exclusive with `--batch`. The default
incremental and batch entrypoints retain their existing behavior. Toolchain,
cache, and matched ThinLTO options are the same as above.

This Linux reference explicitly checks `close(2)` after input parsing and report
flushing, including error paths, rather than relying on Rust's infallible `Drop`.
A close failure supersedes an earlier result; failed flushes still close the
descriptor. It does not retry Linux `close`, request `fsync`, or promise atomic
report replacement. Invalid UTF-8 file contents are covered; invalid UTF-8
process arguments remain outside the common input domain.

The filesystem oracle checks 196 cases against complete directory snapshots,
including malformed payload recovery, all prior graph/parser cases, Unicode,
missing files, directories, embedded NUL paths, line boundaries, report/input
aliases, and terminal errors. The [manifest benchmark](../../manifest_benchmark.py)
also retains a separate Linux `/dev/full` integration check for terminal report
failure, with empty stdout and unchanged inputs. Its timing phase
uses separate three-engine groups for unchanged-program regression controls and
the new valid/mixed manifest workloads. It creates fixtures outside the clock,
then measures fresh processes through file close; recently written inputs have
warm page-cache contents. Every run must match all report bytes and filesystem
effects, as well as status, stdout, and stderr.
