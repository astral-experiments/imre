"""Validate the unchanged dependency-planner package against an independent oracle."""

import argparse
import graphlib
import itertools
import json
import random
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "dependency_planner"
ENTRY = PACKAGE / "__main__.py"
BOOTSTRAP = (
    "import runpy, sys; "
    "sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['dependency_planner']; "
    "runpy.run_module('dependency_planner', run_name='__main__', alter_sys=True)"
)


@dataclass(frozen=True)
class TaskSpec:
    name: str
    duration: int
    dependencies: tuple[int, ...] = ()


@dataclass(frozen=True)
class Case:
    label: str
    tasks: tuple[TaskSpec, ...]
    lookup: str
    stdin_override: str | None = None


def package_sources() -> list[Path]:
    return sorted(PACKAGE.rglob("*.py"))


def python_command(python: str) -> list[str]:
    # -I deliberately ignores ambient PYTHONPATH. This bootstrap adds only the
    # explicit package root and uses the public equivalent of `python -m`.
    return [python, "-I", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def input_text(tasks: tuple[TaskSpec, ...], lookup: str) -> str:
    lines = [str(len(tasks))]
    for task in tasks:
        lines.append(
            " ".join(
                (task.name, str(task.duration), *(str(i) for i in task.dependencies))
            )
        )
    lines.append(lookup)
    return "\n".join(lines) + "\n"


def expected_output(tasks: tuple[TaskSpec, ...], lookup: str) -> str:
    """Derive the complete result without the example's heap or successor lists.

    graphlib independently detects cycles and provides an order for computing
    longest dependency times. A separate exhaustive ready-set scan defines the
    required smallest-ID reporting order. The oracle never imports the package.
    """
    count = len(tasks)
    for task_id, task in enumerate(tasks):
        if task.duration < 0:
            return f"ERROR\nnegative-duration\n{task_id}\n{task.duration}\n"
        for dependency in task.dependencies:
            if not 0 <= dependency < count:
                return f"ERROR\nmissing-dependency\n{task_id}\n{dependency}\n"
    graph = {i: set(task.dependencies) for i, task in enumerate(tasks)}
    try:
        independent_order = list(graphlib.TopologicalSorter(graph).static_order())
        has_cycle = False
    except graphlib.CycleError:
        independent_order = []
        has_cycle = True
    pending = set(range(count))
    completed: set[int] = set()
    order = []
    while pending:
        ready = [i for i in pending if graph[i] <= completed]
        if not ready:
            assert has_cycle
            return f"ERROR\ncycle\n{min(pending)}\n-1\n"
        task_id = min(ready)
        pending.remove(task_id)
        completed.add(task_id)
        order.append(task_id)
    assert not has_cycle

    starts = [0] * count
    finishes = [0] * count
    parents = [-1] * count
    for task_id in independent_order:
        starts[task_id] = max((finishes[i] for i in graph[task_id]), default=0)
        finishes[task_id] = starts[task_id] + tasks[task_id].duration
        parents[task_id] = min(
            (i for i in graph[task_id] if finishes[i] == starts[task_id]),
            default=-1,
        )
    prerequisites = {i for dependencies in graph.values() for i in dependencies}
    sinks = set(range(count)) - prerequisites
    end = min(sinks, key=lambda i: (-finishes[i], i)) if sinks else -1
    makespan = finishes[end] if end != -1 else 0
    path = []
    cursor = end
    while cursor != -1:
        path.append(cursor)
        cursor = parents[cursor]
    path.reverse()
    assert sum(tasks[i].duration for i in path) == makespan
    assert all(parent in graph[child] for parent, child in itertools.pairwise(path))

    lines = ["OK", "PLAN", str(count)]
    for task_id in order:
        task = tasks[task_id]
        lines.extend(
            (
                task.name,
                str(task_id),
                str(task.duration),
                str(starts[task_id]),
                str(finishes[task_id]),
            )
        )
    lines.extend(("MAKESPAN", str(makespan), "CRITICAL", str(len(path))))
    lines.extend(str(i) for i in path)
    lines.extend(("PATH", " -> ".join(tasks[i].name for i in path), "LOOKUP"))
    found = next((task for task in tasks if task.name == lookup), None)
    if found is None:
        lines.extend(("MISSING", lookup))
    else:
        lines.extend(("FOUND", found.name, str(found.duration)))
    return "\n".join(lines) + "\n"


def make_workload(size: int, seed: int, shape: str = "layered") -> tuple[TaskSpec, ...]:
    """Produce reproducible DAG inputs with IDs independent of topological rank."""
    rng = random.Random(seed)
    ids = list(range(size))
    rng.shuffle(ids)
    tasks = [TaskSpec(f"task_{i:05d}", rng.randrange(1, 101)) for i in range(size)]
    for rank, task_id in enumerate(ids):
        if shape == "wide" or rank == 0:
            dependencies = ()
        elif shape == "chain":
            dependencies = (ids[rank - 1],)
        elif shape == "layered":
            candidates = ids[max(0, rank - 40) : rank]
            fan_in = 0 if rank % 17 == 0 else min(len(candidates), rng.randrange(1, 5))
            dependencies = tuple(sorted(rng.sample(candidates, fan_in)))
        else:
            raise ValueError(f"unknown graph shape: {shape}")
        task = tasks[task_id]
        tasks[task_id] = TaskSpec(task.name, task.duration, dependencies)
    return tuple(tasks)


def cases(random_cases: int = 80) -> list[Case]:
    huge = 10**90 + 123
    result = [
        Case("empty", (), "missing"),
        Case("empty-query", (), ""),
        Case("one-zero", (TaskSpec("zero", 0),), "zero"),
        Case("one-missing-lookup", (TaskSpec("build", 7),), "absent"),
        Case(
            "diamond",
            (
                TaskSpec("fetch", 2),
                TaskSpec("compile", 3, (0,)),
                TaskSpec("lint", 3, (0,)),
                TaskSpec("publish", 5, (1, 2)),
            ),
            "publish",
        ),
        Case(
            "disconnected-forward-edges",
            (
                TaskSpec("ship", 3, (2,)),
                TaskSpec("fetch", 4),
                TaskSpec("build", 5, (1,)),
                TaskSpec("docs", 12),
            ),
            "build",
        ),
        Case(
            "zero-duration-path",
            (TaskSpec("a", 0), TaskSpec("b", 0, (0,)), TaskSpec("c", 0, (1,))),
            "c",
        ),
        Case(
            "zero-duration-predecessor-tie",
            (
                TaskSpec("sink", 0, (2, 1)),
                TaskSpec("lower", 0),
                TaskSpec("higher", 0),
            ),
            "sink",
        ),
        Case(
            "terminal-tie",
            (
                TaskSpec("first", 5, (2,)),
                TaskSpec("second", 5, (2,)),
                TaskSpec("root", 0),
            ),
            "first",
        ),
        Case("duplicate-dependency", (TaskSpec("a", 2), TaskSpec("b", 3, (0, 0))), "b"),
        Case(
            "duplicate-label-first-match",
            (TaskSpec("build", 2), TaskSpec("build", 7, (0,))),
            "build",
        ),
        Case(
            "unicode-labels",
            (TaskSpec("编译", 2), TaskSpec("πακέτο", 3, (0,)), TaskSpec("🙂", 1, (1,))),
            "πακέτο",
        ),
        Case(
            "huge-integer-chain",
            (
                TaskSpec("a", huge),
                TaskSpec("b", huge + 1, (0,)),
                TaskSpec("c", huge * 2, (1,)),
            ),
            "b",
        ),
        Case("self-cycle", (TaskSpec("a", 1, (0,)),), "a"),
        Case("two-cycle", (TaskSpec("a", 1, (1,)), TaskSpec("b", 1, (0,))), "a"),
        Case(
            "cycle-with-blocked-dependent",
            (
                TaskSpec("blocked", 1, (1,)),
                TaskSpec("a", 2, (2,)),
                TaskSpec("b", 3, (1,)),
                TaskSpec("free", 0),
            ),
            "free",
        ),
        Case("missing-positive", (TaskSpec("a", 1, (1,)),), "a"),
        Case("missing-negative", (TaskSpec("a", 1, (-1,)),), "a"),
        Case("missing-huge", (TaskSpec("a", 1, (huge,)),), "a"),
        Case("negative-duration", (TaskSpec("a", -huge),), "a"),
        Case("duration-before-own-dependency-error", (TaskSpec("a", -1, (2,)),), "a"),
        Case(
            "earlier-dependency-before-later-duration-error",
            (TaskSpec("a", 1, (2,)), TaskSpec("b", -1)),
            "a",
        ),
        Case(
            "whitespace",
            (TaskSpec("fetch", 2), TaskSpec("build", 3, (0,))),
            "build",
            "2\n  fetch\t2  \n\tbuild  3\t0 \n  build\t\n",
        ),
        Case(
            "unicode-and-control-whitespace",
            (TaskSpec("fetch", 2), TaskSpec("build", 3, (0,))),
            "build",
            "2\nfetch\x1c2\nbuild\u00a03\x1f0\n\u2003build\u3000\n",
        ),
    ]
    rng = random.Random(20261017)
    for index in range(random_cases):
        count = rng.randrange(0, 25)
        ids = list(range(count))
        rng.shuffle(ids)
        tasks = [TaskSpec(f"job_{i}", rng.randrange(0, 20)) for i in range(count)]
        for rank, task_id in enumerate(ids):
            dependencies = tuple(i for i in ids[:rank] if rng.random() < 0.18)
            duration = tasks[task_id].duration + (huge if index % 11 == 0 else 0)
            tasks[task_id] = TaskSpec(tasks[task_id].name, duration, dependencies)
        lookup = (
            tasks[rng.randrange(count)].name if count and index % 2 == 0 else "absent"
        )
        result.append(Case(f"random-dag-{index}", tuple(tasks), lookup))
        if count >= 2 and index % 4 == 0:
            a, b = rng.sample(range(count), 2)
            cyclic = list(tasks)
            cyclic[a] = TaskSpec(
                tasks[a].name, tasks[a].duration, tasks[a].dependencies + (b,)
            )
            cyclic[b] = TaskSpec(
                tasks[b].name, tasks[b].duration, tasks[b].dependencies + (a,)
            )
            result.append(Case(f"random-cycle-{index}", tuple(cyclic), lookup))
        if count and index % 7 == 0:
            bad = list(tasks)
            task_id = rng.randrange(count)
            task = bad[task_id]
            bad[task_id] = TaskSpec(
                task.name, task.duration, task.dependencies + (count + index,)
            )
            result.append(Case(f"random-missing-{index}", tuple(bad), lookup))
    return result


def execute(command: list[str], stdin: str) -> str:
    result = subprocess.run(
        command, input=stdin, capture_output=True, text=True, timeout=120, check=False
    )
    if result.returncode or result.stderr:
        raise RuntimeError(
            f"{Path(command[0]).name} failed ({result.returncode}): {result.stderr}"
        )
    return result.stdout


def validate(binary: Path | None, python: str, random_cases: int = 80) -> dict:
    counts = {
        "inputs": 0,
        "valid_plans": 0,
        "cycles": 0,
        "missing_dependencies": 0,
        "negative_durations": 0,
    }
    task_count = 0
    for case in cases(random_cases):
        stdin = (
            case.stdin_override
            if case.stdin_override is not None
            else input_text(case.tasks, case.lookup)
        )
        expected = expected_output(case.tasks, case.lookup)
        reference = execute(python_command(python), stdin)
        if reference != expected:
            raise AssertionError(
                f"CPython/oracle disagreement in {case.label}:\n{reference!r}\n!=\n{expected!r}"
            )
        if binary is not None:
            native = execute([str(binary)], stdin)
            if native != expected:
                raise AssertionError(
                    f"Imre/oracle disagreement in {case.label}:\n{native!r}\n!=\n{expected!r}"
                )
        counts["inputs"] += 1
        task_count += len(case.tasks)
        if expected.startswith("OK\n"):
            counts["valid_plans"] += 1
        elif expected.startswith("ERROR\ncycle\n"):
            counts["cycles"] += 1
        elif expected.startswith("ERROR\nmissing-dependency\n"):
            counts["missing_dependencies"] += 1
        else:
            counts["negative_durations"] += 1
    return {
        **counts,
        "input_tasks": task_count,
        "native_checked": binary is not None,
        "oracle": "graphlib cycle detection, exhaustive ready-set ordering, and independent dependency maximums",
        "checks": "complete output, topological order, earliest starts/finishes, terminal critical path, and named lookup",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--random-cases", type=int, default=80)
    args = parser.parse_args()
    if args.random_cases < 0:
        parser.error("--random-cases must be nonnegative")
    binary = args.binary.resolve(strict=True) if args.binary is not None else None
    print(json.dumps(validate(binary, args.python, args.random_cases), indent=2))


if __name__ == "__main__":
    main()
