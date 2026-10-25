# Concrete-layout comparisons

The [recorded comparison and profiles](results/2026-10-18-concrete-layouts.md)
publish the final measurements, raw samples, build provenance, and validation.

`layout_benchmark.py` compares three unchanged applications independently:

| Group | Captured workloads | Engines |
| --- | --- | --- |
| Incremental planner | Five sparse/all/boundary cases | CPython, baseline Imre, current Imre, Rust BigInt |
| Batch planner | Six valid/mixed cases | The same four engines |
| Manifest planner | Six valid/mixed filesystem cases | The same four engines |

The inputs and complete expected outputs come from the prior manifest and
exception comparison records. They are checked again with the independent
oracles before use. The Python packages and original Rust reference executables
stay unchanged. Historical Rust source snapshots are retained with their build
records; adding later application entrypoints does not require rebuilding the
older controls.

Each invocation compiles the three current Imre programs once at O3, then runs
the full incremental, batch, and manifest validators against all four engines.
Manifest validation includes the separate Linux `/dev/full` application check.
Use `--validate-only` to stop after these checks without collecting benchmark
samples.

The default is 32 measured rounds and two warmup rounds. A four-row Williams
cycle gives each engine every position once and every directed adjacent pair
once per cycle. Cross-round predecessors are not balanced. Each sample starts
a fresh process pinned to the selected CPU; timing under ptrace is refused.
All timings, statuses, stderr, output lengths and hashes are retained after
complete output comparison. No samples are discarded.
The record includes before/after tracer checks and an allowlist of locale,
Python, allocator, loader and sanitizer environment settings. Every engine
inherits the same environment; CPython's `-I` mode ignores Python environment
settings.

Manifest runs get fresh isolated working directories. Input fixture creation,
oracle work, full directory comparisons, hashing and deletion are outside the
timer. The timed process includes file opens, reads, writes, flushes and closes.
Inputs are page-cache warm and output is not fsynced. These are whole-process
application comparisons on a shared machine, not cold-storage measurements or
isolated layout costs. Allocation and instruction profiles should be reported
separately from elapsed time.

## Preserved baseline

`--baseline-record` selects a JSON index whose paths are relative to its parent.
The initial bundle is `benchmarks/local/layout-baseline/baseline.json`, frozen
from clean revision `32dd63afa5599cb8d61d49e2730b3f6eeae87755` before layout changes.
It contains:

- A tracked-source archive and per-file hashes, plus the compiler/runtime build
  record and real copies of the compiler and ordinary/ThinLTO archives.
- Ordinary and ThinLTO incremental, batch and manifest executables with LLVM IR.
  Incremental/manifest executables are the exact previously measured artifacts;
  batch was compiled with the same frozen compiler/runtime generation and passed
  the full 194-case oracle in both modes.
- The captured manifest/exception input records and all six Rust BigInt
  executables, their build records, source snapshots and build helpers.

The index seals each file's relative path, SHA256 and byte length. The harness
cross-checks executable hashes against their original build records, Python
source identity, compiler/runtime generation, effective target, optimization
mode, and Rust source/build-helper contents. It hashes all inputs again after
the run and protects them from output collisions, including hardlinks.

The baseline bundle is a local artifact, not a checked-in binary distribution.
Published comparison records retain its index and build provenance. A new
baseline must have its own build records and index; do not relabel a rebuilt
binary as an older measured executable.

## Run

Use separate ordinary and ThinLTO output directories and records. For example,
after freezing current tools and waiting until builds/profilers are idle:

```sh
python3 benchmarks/layout_benchmark.py \
  --compiler benchmarks/local/layout-tools-v2/imre \
  --runtime benchmarks/local/layout-tools-v2/libimre_runtime.a \
  --imre-build-record benchmarks/local/layout-tools-v2/build-record.json \
  --baseline-record benchmarks/local/layout-baseline/baseline.json \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --cpu 0 --samples 32 --warmups 2 \
  --artifact-dir benchmarks/local/layout-normal-final \
  --output benchmarks/local/layout-normal-final.json

python3 benchmarks/layout_benchmark.py \
  --compiler benchmarks/local/layout-tools-v2/imre \
  --runtime benchmarks/local/layout-tools-v2/libimre_runtime_lto.a \
  --imre-build-record benchmarks/local/layout-tools-v2/build-record.json \
  --baseline-record benchmarks/local/layout-baseline/baseline.json \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --lto-linker /path/to/ld.lld \
  --cpu 0 --samples 32 --warmups 2 \
  --artifact-dir benchmarks/local/layout-lto-final \
  --output benchmarks/local/layout-lto-final.json
```

The `ld.lld` path must invoke the actual matching LLVM linker backend (a symlink
named `ld.lld` is supported). Its lexical filename is preserved for invocation,
while hashing follows the link. ThinLTO uses the existing 1000 import/inline
budgets and requires matching Rust reference settings; ordinary builds use the
original ordinary Rust controls. Rust and Clang backend versions remain
separately recorded and need not be identical.

`--incremental-workload`, `--batch-workload` and `--manifest-workload` can select
captured labels for diagnostic runs. Final comparisons use all workloads.

## Allocation and instruction profiles

`layout_profile.py` verifies the comparison, build record, baseline bundle and
exact executable/IR hashes before profiling. By default it profiles the captured
16K layered/all and wide/all incremental workloads under baseline Imre, current
Imre and the preserved Rust BigInt control. It delegates collection and parsing
to `dependency_profile.py`, retaining raw traces, logs, full stdout, hashes,
commands, function/edge counts and allocation stacks.

```sh
python3 benchmarks/layout_profile.py \
  --record benchmarks/local/layout-normal-final.json \
  --artifact-dir benchmarks/local/layout-normal-final \
  --baseline-record benchmarks/local/layout-baseline/baseline.json \
  --build-record benchmarks/local/layout-tools-v2/build-record.json \
  --output benchmarks/local/layout-profile-normal \
  --python python3.14 --clang clang-18 --cpu 0 --probe
```

For ThinLTO, select the corresponding comparison/artifact directory and pass
the same `--lto-linker` used for its build. `--valgrind` and `--valgrind-lib`
support an explicitly installed profiler; otherwise the shared driver discovers
Valgrind. Run outside ptrace, with no timing benchmark running concurrently.

`--probe` additionally compiles the identical
[`fixtures/layout_storage.py`](fixtures/layout_storage.py) with both frozen
compiler/runtime pairs, validates closed-form checksums against CPython, and
runs DHAT at counts 1,000 and 10,000. The three separate paths retain and read
present integer/object optionals, fixed-layout objects, and typed sequences.
String constants are shared to avoid measuring repeated string construction.
Use `--skip-applications --probe` for this allocation experiment alone.

The storage fixture is a mechanism probe, not a new timed application. DHAT
reports requested allocation traffic and live heap, not RSS. Callgrind counts
executed instructions, not hardware cycles or elapsed-time percentages. No
hardware counter measurement is implied. The profile record identifies whether
its input record came from validation only or from the final timed comparison.
