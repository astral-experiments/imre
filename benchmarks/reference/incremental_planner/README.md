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
