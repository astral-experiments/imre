# Dependency planner profiling

`dependency_profile.py` profiles prebuilt native executables with Callgrind and
DHAT. It reuses the complete captured inputs and expected outputs in a dependency
benchmark or comparison JSON. It does not build executables or run performance
benchmarks.

```sh
python3 benchmarks/dependency_profile.py \
  --record benchmarks/local/dependency-comparison.json \
  --binary baseline=benchmarks/local/dependency-planner-buffered/dependency_planner \
  --binary optimized=benchmarks/local/dependency-planner-optimized/dependency_planner \
  --binary rust_bigint=benchmarks/local/rust-reference/planner-bigint \
  --binary rust_i64=benchmarks/local/rust-reference/planner-i64 \
  --sizes 16384 --shapes layered wide \
  --output benchmarks/local/dependency-cpu-profile/comparison
```

Select the actual artifact paths from the timing record. Repeat `--binary
NAME=PATH` for any subset of implementations. `--build-record NAME=JSON` attaches
optional build provenance and checks that it contains that executable's hash.
The input record's own artifact hashes are also checked for membership and
recorded; a newly optimized executable can intentionally differ from those
earlier artifacts. Every executable is hashed before and after profiling.

The driver requires Linux, Python 3.10 or newer, and Valgrind with Callgrind and
DHAT. It discovers `valgrind` on `PATH`, then checks the local Relar tool cache.
For an extracted package, pass both locations explicitly:

```sh
python3 benchmarks/dependency_profile.py \
  --record benchmarks/local/dependency-comparison.json \
  --binary optimized=benchmarks/local/dependency-planner-optimized/dependency_planner \
  --valgrind /path/to/extracted/usr/bin/valgrind \
  --valgrind-lib /path/to/extracted/usr/libexec/valgrind \
  --sizes 16384 --output benchmarks/local/dependency-cpu-profile/optimized
```

`--tools callgrind` or `--tools dhat` selects one profiler. `--cpu` defaults to
the first allowed CPU, and `--timeout` defaults to 300 seconds per process.
Omitting `--sizes` and `--shapes` selects every captured workload. Run diagnostic
profiling separately from clean wall-clock benchmarks.

The output directory must be new. It retains the input record, exact harness
source, profiler versions and hashes, binary hashes, commands, complete stdin and
stdout, stderr, profiler logs, raw traces, and a file manifest. Every profiled
execution must exit successfully, emit no program stderr, and match the complete
expected stdout byte for byte. Partial evidence is retained if a run fails.

`summary.json` includes all exclusive instruction counts and call edges, plus all
allocation stacks, total requested bytes/blocks, peak live bytes/blocks, and mean
allocation lifetimes. Rebuild a summary without executing anything:

```sh
python3 benchmarks/dependency_profile.py \
  --summarize benchmarks/local/dependency-cpu-profile/comparison
```

Callgrind counts **executed machine instructions**, not hardware CPU cycles or
elapsed-time shares. Exclusive function counts reconcile exactly with its
reported total. Inclusive call costs overlap and cannot be added indiscriminately.
Inlining and shared symbol addresses can combine source-level operations; inspect
callers and disassembly before assigning costs to a particular operation.

DHAT replaces allocator routines. Its byte totals describe requested heap
allocation traffic and live allocations, rather than process RSS. Lifetimes use
executed instructions as their unit. Instrumented elapsed times are preserved
only as diagnostics and never contribute to reported performance speedups.
