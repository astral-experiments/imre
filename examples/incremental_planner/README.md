# Named incremental rebuild planner

This sibling of the original dependency planner resolves target names with a
typed dictionary, finds transitively affected targets with sets, and iterates
lists, sets, dictionary items, and `enumerate` results directly. The same package
runs on CPython and compiles with Imre; it imports no standard-library modules.

Input contains a nonnegative target count, that many target lines, a nonnegative
changed-name count, and that many changed-name lines:

```text
5
fetch 2
compile 3 fetch
test 2 compile
docs 4
release 1 test docs
1
compile
```

Each target line is `NAME DURATION [DEPENDENCY_NAME ...]`. Names are nonempty
whitespace-free tokens; forward references are allowed. Durations are arbitrary
precision integers. Repeated dependencies and changed names are deduplicated.
Input framing and numeric syntax must be valid; malformed input raises a runtime
error. Extra input after the counted lines is ignored.

Graph validation uses these phases, in this exact order:

1. Duplicate target names: first duplicate occurrence in target input order.
2. Negative durations: first target with a negative duration.
3. Unknown dependencies: first target and dependency token in input order.
4. Unknown changed names: first changed-name line referring to no target.
5. Cycles: lowest input ID still blocked after all ready targets are removed.

Every phase examines the whole graph, including when the change list is empty.
An error prints four lines: `ERROR`, its kind (`duplicate-name`,
`negative-duration`, `unknown-dependency`, `unknown-change`, or `cycle`), the
zero-based target ID or changed-line index, and the offending name or negative
duration. Cycle details identify the lowest blocked target; that target may be
a downstream dependent rather than a member of the cycle.

The changed targets and all their transitive dependents are rebuilt. Unaffected
prerequisites are already built and contribute zero rebuild time. Among ready
affected targets, the smallest input ID is chosen first. Start times are the
maximum finish of affected prerequisites, and finishes add the target duration.
Hash iteration order cannot affect the reported order or times.

A successful report starts with `OK`, `CHANGED`, the unique changed count,
`AFFECTED`, the affected count, `PLAN`, and the affected count again. Each rebuilt
target contributes five lines: name, ID, duration, start, finish. The final two
lines are `MAKESPAN` and its value. For the example above, the rebuilt names are
`compile`, `test`, `release`, with finishes 3, 5, and 6. The independent oracle
checks this entire report, including names and every time, rather than a checksum.
The validator has 155 graph cases and 17 separate parser cases. Malformed framing
checks require missing input, malformed integer tokens, and invalid row counts to
produce the same fatal error category as CPython. Input parsing completes before
the graph-validation phases begin.

From the repository root:

```sh
PYTHONPATH=examples python3 -m incremental_planner < input.txt
target/release/imre build examples/incremental_planner/__main__.py \
  --module-root examples --runtime target/release/libimre_runtime.a \
  -o benchmarks/local/incremental_planner
python3 benchmarks/incremental_validate.py \
  --binary benchmarks/local/incremental_planner --python python3.14
```

The Rust reference and whole-process comparison are documented in
[the reference guide](../../benchmarks/reference/incremental_planner/README.md).
Correctness checks run before timing. Four-engine comparisons use 32 measured
rounds, two warmups, pinned CPU affinity, fresh processes, and a balanced Williams
order; ordinary and ThinLTO comparisons use corresponding Rust build modes.
