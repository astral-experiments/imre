# Dependency planner

This first-party package schedules tasks with dependency constraints and unlimited workers. It produces a deterministic topological order, each task's earliest start and finish, the total completion time, a critical path, and a named-task lookup. The exact same package runs under CPython or compiles to an independent Imre executable.

The implementation uses static modules, typed classes and methods, lists of task objects, nested successor lists, whitespace-tokenized string lists, fixed tuples, string concatenation, and an optional task return narrowed with `is None`. Dependency edges are integer IDs rather than object references: the semantic graph may contain cycles, while the runtime object graph remains acyclic. No compiler-specific helper replaces the scheduling algorithm.

## Input and semantics

Input contains a task count, that many task lines, and one final lookup-name line. Each task line is `name duration dependency-ID ...`. IDs are zero-based input positions; forward references are allowed. Names are nonempty whitespace-free labels, and durations are nonnegative arbitrary-precision integers. Repeated dependency IDs are accepted without changing the result. Labels need not be unique; named lookup returns the first matching input task. An empty lookup line requests a missing result.

For example:

```text
4
fetch 2
compile 3 0
lint 3 0
publish 5 1 2
publish
```

When several tasks are ready, the planner selects the smallest ID. Earliest finish is duration plus the maximum finish of a task's dependencies, with zero for tasks without dependencies. The critical path ends at a terminal task with the latest finish; ties choose the smallest terminal ID. Ties between critical predecessors also choose the smallest ID. The example therefore completes at time 10, with critical path `fetch -> compile -> publish`.

The scheduler reports `missing-dependency`, `negative-duration`, or `cycle` as normal graph-validation results. Duration and dependency checks occur in input order before scheduling. A cycle report identifies the smallest unscheduled task ID, which may be blocked downstream of a cycle; it is not a cycle witness. Structurally malformed input, such as an invalid integer or missing duration token, raises a language input/assertion error instead.

## Complete output

A successful report starts with `OK`, `PLAN`, and the task count. Each task then has five lines, in the selected topological order: name, ID, duration, earliest start, and earliest finish. The remaining sections are:

```text
MAKESPAN
<total completion time>
CRITICAL
<path length>
<one task ID per path entry>
PATH
<names joined with " -> ">
LOOKUP
FOUND
<matched name>
<matched duration>
```

A missing lookup ends with `LOOKUP`, `MISSING`, and the queried name. An empty graph has completion time zero, an empty critical path, and a missing lookup. An invalid graph prints four lines: `ERROR`, the error code, the task ID, and the offending duration/dependency ID; a cycle uses `-1` for the final detail.

## Run and validate

From the repository root, CPython can run the unmodified package with:

```sh
PYTHONPATH=examples python3.14 -m dependency_planner < problem.txt
```

The harness uses `-I` with an explicit package-root bootstrap and `runpy.run_module`, so ambient Python paths cannot change which package executes. It runs the package as `__main__`, matching `-m` behavior, and permits normal imported-module bytecode caching.

Build and validate the native executable with the compiler's module root set to `examples`:

```sh
cargo build --release --workspace --locked
mkdir -p benchmarks/local/dependency-planner
target/release/imre build examples/dependency_planner/__main__.py \
  --module-root examples \
  --runtime target/release/libimre_runtime.a \
  -o benchmarks/local/dependency-planner/dependency_planner
python3.14 benchmarks/dependency_validate.py \
  --binary benchmarks/local/dependency-planner/dependency_planner \
  --python python3.14
```

On an Ohm-configured development machine, use `cargo +ohm` for the workspace build and the compiler/runtime paths from that checkout's `CARGO_TARGET_DIR`. Without `--binary`, the validator checks the CPython package against the independent oracle, which is useful before native compilation is available.

The oracle never imports the example. It uses `graphlib` for cycle detection and an independent topological order, exhaustive ready-set scanning for the required smallest-ID order, and predecessor maximums for earliest times. It checks the complete report, including critical-path and lookup output. The default suite contains 135 hand-written and deterministic random inputs: empty, disconnected, diamond and cyclic graphs; forward IDs; zero and very large durations; critical-path ties; validation-error precedence; missing dependencies; duplicate edges and labels; Unicode names; and ASCII, Unicode, and control whitespace between tokens.

## Benchmark

Once correctness passes and concurrent builds have stopped:

```sh
python3.14 benchmarks/dependency_benchmark.py \
  --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a \
  --python python3.14 \
  --output benchmarks/local/dependency-planner.json
```

Defaults cover 128, 512, and 2,048 tasks in layered DAGs and independent-task graphs, with two warmup pairs and twelve measured pairs per input. `--shapes chain` also exercises a long serial critical path. IDs are shuffled independently of dependency order, and all input text is retained. `--sizes`, `--shapes`, `--seed`, `--samples`, `--warmups`, and `--cpu` select other cases; even sample counts balance which implementation runs first.

The harness compiles once at LLVM O3, validates against the oracle and CPython, and compares complete output on every run. It shares the existing benchmark's single-CPU affinity, tracing refusal, fresh-process timer, metadata probe, and summary calculations. Timings include startup, Python package imports, input parsing, scheduling, and the entire human-readable report, including printing all tasks. They exclude AOT compilation, correctness-oracle execution, and warmups; AOT compilation time is recorded separately and excludes the Rust workspace build. The default process timeout is 120 seconds; the validator independently uses the same limit.

JSON retains every raw duration, execution order, exit status, and stderr, plus the full input and complete expected output once per workload. Each warmup and sample stores its stdout SHA256 and UTF-8 byte length after an exact comparison with that expected output; hashing and comparison are outside the timed region. This preserves evidence for every execution without repeating a large report in every sample. Records also include hashes of every package source file and relevant tool/artifact, Git revision and dirty status, interpreter/build metadata, and CPU/OS details.

Medians, observed ranges, and quartiles describe these runs rather than providing confidence intervals. Shared-host scheduling, CPU frequency, startup, and report size affect results; the measurements establish behavior for these dependency-planner inputs, not a general Python speedup. The default cases are quick to reproduce; `--sizes 128 2048 16384 --shapes layered wide` additionally exposes scaling across larger graphs.

To compare a new native build with a preserved baseline in the same session, supply both the baseline executable and its original benchmark record:

```sh
python3.14 benchmarks/dependency_benchmark.py \
  --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a \
  --baseline benchmarks/local/dependency-planner-final/dependency_planner \
  --baseline-record benchmarks/results/2026-10-17-dependency-planner-baseline.json \
  --python python3.14 --sizes 128 2048 16384 --shapes layered wide \
  --artifact-dir benchmarks/local/dependency-planner-comparison \
  --output benchmarks/local/dependency-planner-comparison.json
```

The baseline is never rebuilt. Its executable hash and all package source hashes must match the supplied record before compilation or validation proceeds; output paths must not overwrite either baseline input. The result retains the record's hash, revision, source/tool/artifact hashes, tool versions, and compilation provenance. Both native executables independently pass all 135 oracle cases before timing, and every subsequent run must match complete output. Source files, tools, both native executables, and the baseline record are checked again after measurement.

In this mode each warmup or sample is a triplet. The six execution orders repeat in a cycle, with adjacent orders reversing each other; the default twelve measured triplets use every order twice. Use a multiple of six samples to balance every implementation's execution position and immediate predecessor. The default two warmup triplets prime every implementation but do not cover all six orders. Summaries retain the CPython/Imre ratio and add baseline/Imre and CPython/baseline ratios, each computed from medians in the same session. Omitting both baseline arguments retains the two-way paired benchmark.
