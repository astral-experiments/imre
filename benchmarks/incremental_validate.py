"""Independent oracle and complete-output checks for the incremental planner."""

import argparse
import graphlib
import hashlib
import heapq
import json
import random
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "incremental_planner"
ENTRY = PACKAGE / "__main__.py"
BOOTSTRAP = (
    "import runpy, sys; "
    "sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['incremental_planner']; "
    "runpy.run_module('incremental_planner', run_name='__main__', alter_sys=True)"
)
I64_MIN = -(1 << 63)
I64_MAX = (1 << 63) - 1


@dataclass(frozen=True)
class TaskSpec:
    name: str
    duration: int
    dependencies: tuple[str, ...] = ()


@dataclass(frozen=True)
class Case:
    label: str
    tasks: tuple[TaskSpec, ...]
    changes: tuple[str, ...]
    stdin_override: str | None = None


def package_sources() -> list[Path]:
    return sorted(PACKAGE.rglob("*.py"))


def python_command(python: str) -> list[str]:
    return [python, "-I", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def input_text(tasks: tuple[TaskSpec, ...], changes: tuple[str, ...]) -> str:
    rows = [str(len(tasks))]
    rows.extend(
        " ".join((task.name, str(task.duration), *task.dependencies)) for task in tasks
    )
    rows.extend((str(len(changes)), *changes))
    return "\n".join(rows) + "\n"


def oracle(tasks: tuple[TaskSpec, ...], changes: tuple[str, ...]) -> dict:
    """Use graphlib for topology and a fixed-point scan for affected closure.

    The oracle never imports the compiled package or uses its reverse adjacency,
    indegree bookkeeping, handwritten heap, or queue-based closure algorithm.
    """

    def error(kind: str, index: int, detail: str | int) -> dict:
        return {
            "kind": kind,
            "output": f"ERROR\n{kind}\n{index}\n{detail}\n",
            "order": [],
            "starts": [],
            "finishes": [],
            "changed_count": 0,
        }

    names = {}
    for index, task in enumerate(tasks):
        if task.name in names:
            return error("duplicate-name", index, task.name)
        names[task.name] = index
    for index, task in enumerate(tasks):
        if task.duration < 0:
            return error("negative-duration", index, task.duration)
    for index, task in enumerate(tasks):
        for name in task.dependencies:
            if name not in names:
                return error("unknown-dependency", index, name)
    for index, name in enumerate(changes):
        if name not in names:
            return error("unknown-change", index, name)
    graph = {
        index: {names[name] for name in task.dependencies}
        for index, task in enumerate(tasks)
    }
    full = graphlib.TopologicalSorter(graph)
    try:
        full.prepare()
    except graphlib.CycleError:
        completed = set()
        while full.is_active():
            ready = full.get_ready()
            completed.update(ready)
            full.done(*ready)
        blocked = min(set(graph) - completed)
        return error("cycle", blocked, tasks[blocked].name)

    changed = {names[name] for name in changes}
    affected = set(changed)
    while True:
        expanded = affected | {
            index for index, dependencies in graph.items() if dependencies & affected
        }
        if expanded == affected:
            break
        affected = expanded
    partial = graphlib.TopologicalSorter(
        {index: graph[index] & affected for index in affected}
    )
    partial.prepare()
    ready = list(partial.get_ready())
    heapq.heapify(ready)
    order = []
    starts = [0] * len(tasks)
    finishes = [0] * len(tasks)
    while ready:
        index = heapq.heappop(ready)
        order.append(index)
        starts[index] = max(
            (finishes[parent] for parent in graph[index] & affected), default=0
        )
        finishes[index] = starts[index] + tasks[index].duration
        partial.done(index)
        for child in partial.get_ready():
            heapq.heappush(ready, child)
    assert set(order) == affected
    assert all(
        starts[index] >= finishes[parent]
        for index in order
        for parent in graph[index] & affected
    )
    lines = [
        "OK",
        "CHANGED",
        str(len(changed)),
        "AFFECTED",
        str(len(order)),
        "PLAN",
        str(len(order)),
    ]
    for index in order:
        lines.extend(
            (
                tasks[index].name,
                str(index),
                str(tasks[index].duration),
                str(starts[index]),
                str(finishes[index]),
            )
        )
    lines.extend(("MAKESPAN", str(max(finishes, default=0))))
    return {
        "kind": "OK",
        "output": "\n".join(lines) + "\n",
        "order": order,
        "starts": starts,
        "finishes": finishes,
        "changed_count": len(changed),
    }


def i64_exclusion(tasks: tuple[TaskSpec, ...], expected: dict) -> str | None:
    # Parsing precedes graph validation; an unused huge duration still rejects.
    for index, task in enumerate(tasks):
        if not I64_MIN <= task.duration <= I64_MAX:
            return f"target {index} duration {task.duration} is outside signed 64-bit range"
    for index, finish in enumerate(expected["finishes"]):
        if finish > I64_MAX:
            return f"target {index} rebuild finish {finish} exceeds signed 64-bit range"
    return None


def make_workload(size: int, seed: int, shape: str, changes: str) -> Case:
    """Keep input IDs independent of topology, and sparse changes near its end."""
    rng = random.Random(seed)
    ids = list(range(size))
    rng.shuffle(ids)
    tasks = [TaskSpec(f"target_{i:05d}", rng.randrange(1, 101)) for i in range(size)]
    if shape == "boundary":
        durations = (0, (1 << 62) - 1, 1 << 62, I64_MAX)
        prefixes = ("a" * 23, "é" * 12, "🙂" * 6, "b" * 25)
        tasks = [
            TaskSpec(f"{prefixes[i % 4]}_{i}", durations[i % 4]) for i in range(size)
        ]
    elif shape == "layered":
        for rank, task_id in enumerate(ids):
            candidates = ids[max(0, rank - 48) : rank]
            degree = 0 if rank % 19 == 0 else min(len(candidates), rng.randrange(1, 5))
            dependencies = tuple(
                tasks[i].name for i in sorted(rng.sample(candidates, degree))
            )
            task = tasks[task_id]
            tasks[task_id] = TaskSpec(task.name, task.duration, dependencies)
    elif shape != "wide":
        raise ValueError(f"unknown shape: {shape}")
    if changes == "none" or not tasks:
        changed_names = ()
    elif changes == "all":
        changed_names = tuple(task.name for task in tasks)
    elif changes == "sparse":
        changed_names = (tasks[ids[max(0, size - max(1, size // 32))]].name,)
    else:
        raise ValueError(f"unknown change selection: {changes}")
    return Case(f"{shape}-{size}-{changes}", tuple(tasks), changed_names)


def cases(random_cases: int = 80) -> list[Case]:
    huge = 10**90 + 123
    diamond = (
        TaskSpec("release", 5, ("compile", "lint")),
        TaskSpec("fetch", 2),
        TaskSpec("compile", 3, ("fetch",)),
        TaskSpec("lint", 3, ("fetch",)),
        TaskSpec("docs", 40),
    )
    result = [
        Case("empty", (), ()),
        Case("empty-unknown-change", (), ("missing",)),
        Case("one-zero", (TaskSpec("a", 0),), ("a",)),
        Case("one-no-changes", (TaskSpec("a", 7),), ()),
        Case("diamond-all", diamond, tuple(task.name for task in diamond)),
        Case("diamond-root", diamond, ("fetch",)),
        Case("diamond-partial", diamond, ("compile",)),
        Case("diamond-sink", diamond, ("release",)),
        Case("diamond-disconnected", diamond, ("docs",)),
        Case("duplicate-changes", diamond, ("compile", "compile", "fetch", "fetch")),
        Case(
            "duplicate-dependencies",
            (TaskSpec("a", 2), TaskSpec("b", 3, ("a", "a"))),
            ("a",),
        ),
        Case("duplicate-name", (TaskSpec("a", 2), TaskSpec("a", 3)), ()),
        Case("duplicate-before-negative", (TaskSpec("a", -1), TaskSpec("a", 2)), ()),
        Case("negative-duration", (TaskSpec("a", -huge),), ()),
        Case(
            "negative-before-unknown-dependency",
            (TaskSpec("a", 1, ("missing",)), TaskSpec("b", -2)),
            (),
        ),
        Case("unknown-dependency", (TaskSpec("a", 1, ("missing",)),), ()),
        Case(
            "unknown-dependency-token-order",
            (TaskSpec("a", 1, ("second", "first")),),
            (),
        ),
        Case(
            "unknown-dependency-before-change",
            (TaskSpec("a", 1, ("dependency",)),),
            ("change",),
        ),
        Case("unknown-change", (TaskSpec("a", 1),), ("a", "missing", "other")),
        Case("self-cycle", (TaskSpec("a", 1, ("a",)),), ("a",)),
        Case(
            "cycle-no-changes", (TaskSpec("a", 1, ("b",)), TaskSpec("b", 1, ("a",))), ()
        ),
        Case("unknown-change-before-cycle", (TaskSpec("a", 1, ("a",)),), ("missing",)),
        Case(
            "unaffected-cycle",
            (TaskSpec("free", 1), TaskSpec("a", 1, ("b",)), TaskSpec("b", 1, ("a",))),
            ("free",),
        ),
        Case(
            "lowest-blocked-dependent",
            (
                TaskSpec("blocked", 1, ("a",)),
                TaskSpec("a", 1, ("b",)),
                TaskSpec("b", 1, ("a",)),
            ),
            (),
        ),
        Case(
            "unicode-and-nul",
            (
                TaskSpec("编译", 2),
                TaskSpec("a\0b", 3, ("编译",)),
                TaskSpec("🙂", 4, ("a\0b",)),
            ),
            ("编译",),
        ),
        Case(
            "huge-chain", (TaskSpec("a", huge), TaskSpec("b", huge + 1, ("a",))), ("a",)
        ),
        Case(
            "huge-unaffected", (TaskSpec("a", huge), TaskSpec("b", 3, ("a",))), ("b",)
        ),
        Case("max-i64", (TaskSpec("a", I64_MAX),), ("a",)),
        Case("min-i64-negative", (TaskSpec("a", I64_MIN),), ()),
        Case("max-plus-one", (TaskSpec("a", I64_MAX + 1),), ("a",)),
        Case("min-minus-one", (TaskSpec("a", I64_MIN - 1),), ()),
        Case(
            "finish-overflow",
            (TaskSpec("a", I64_MAX), TaskSpec("b", 1, ("a",))),
            ("a",),
        ),
        Case(
            "unaffected-duration-not-added",
            (TaskSpec("a", I64_MAX), TaskSpec("b", 1, ("a",))),
            ("b",),
        ),
        Case(
            "no-changes-no-finish-overflow",
            (TaskSpec("a", I64_MAX), TaskSpec("b", 1, ("a",))),
            (),
        ),
        Case(
            "cycle-before-finish-overflow",
            (
                TaskSpec("a", I64_MAX),
                TaskSpec("b", 1, ("a",)),
                TaskSpec("cycle", 1, ("cycle",)),
            ),
            ("a",),
        ),
        Case(
            "signs-underscores",
            (TaskSpec("a", I64_MAX),),
            ("a",),
            "+0_1\na +9_223_372_036_854_775_807\n1\na\n",
        ),
        Case(
            "unicode-whitespace",
            (TaskSpec("a", 2), TaskSpec("b", 3, ("a",))),
            ("a",),
            "2\na\x1c2\nb\u00a03\x1fa\n1\n\u2003a\u3000\n",
        ),
        Case("crlf", (TaskSpec("a", 2),), ("a",), "1\r\na 2\r\n1\r\na\r\n"),
    ]
    rng = random.Random(20261017)
    for index in range(random_cases):
        size = rng.randrange(0, 27)
        ids = list(range(size))
        rng.shuffle(ids)
        tasks = [TaskSpec(f"job_{i}", rng.randrange(0, 25)) for i in range(size)]
        for rank, task_id in enumerate(ids):
            dependencies = tuple(
                tasks[i].name for i in ids[:rank] if rng.random() < 0.2
            )
            if dependencies and index % 4 == 0:
                dependencies += dependencies[:1]
            task = tasks[task_id]
            tasks[task_id] = TaskSpec(
                task.name,
                task.duration + (huge if index % 13 == 0 else 0),
                dependencies,
            )
        changed = tuple(task.name for task in tasks if rng.random() < 0.3)
        if changed and index % 5 == 0:
            changed += changed[:1]
        result.append(Case(f"random-dag-{index}", tuple(tasks), changed))
        if tasks and index % 5 == 0:
            task = tasks[0]
            cyclic = [
                TaskSpec(task.name, task.duration, (*task.dependencies, task.name)),
                *tasks[1:],
            ]
            result.append(Case(f"random-cycle-{index}", tuple(cyclic), changed))
        if tasks and index % 7 == 0:
            task = tasks[-1]
            unknown = [
                *tasks[:-1],
                TaskSpec(task.name, task.duration, (*task.dependencies, "missing")),
            ]
            result.append(Case(f"random-unknown-{index}", tuple(unknown), changed))
        if tasks and index % 11 == 0:
            result.append(
                Case(
                    f"random-unknown-change-{index}",
                    tuple(tasks),
                    (*changed, "missing"),
                )
            )
    result.extend(
        (
            make_workload(48, 7, "boundary", "all"),
            make_workload(48, 7, "layered", "none"),
        )
    )
    return result


def parser_cases() -> list[tuple[str, str, str | None, str]]:
    """Keep malformed framing separate from the graph oracle's error results."""
    invalid = [
        ("empty-input", "", "EOFError"),
        ("negative-target-count", "-1\n", "AssertionError"),
        ("missing-target", "1\n", "EOFError"),
        ("missing-duration", "1\na\n0\n", "AssertionError"),
        ("double-underscore-duration", "1\na 1__0\n0\n", "ValueError"),
        ("leading-underscore-duration", "1\na _1\n0\n", "ValueError"),
        ("sign-only-duration", "1\na +\n0\n", "ValueError"),
        ("nondecimal-duration", "1\na 0x10\n0\n", "ValueError"),
        ("negative-change-count", "1\na 1\n-1\n", "AssertionError"),
        ("missing-change", "1\na 1\n1\n", "EOFError"),
        ("empty-change", "1\na 1\n1\n\n", "AssertionError"),
        ("multiple-change-tokens", "1\na 1\n1\na a\n", "AssertionError"),
        ("parse-before-duplicate-validation", "2\na 1\na x\n0\n", "ValueError"),
        ("split-only-whitespace-in-count", "\x1c1\na 2\n1\na\n", "ValueError"),
    ]
    result = [(label, stdin, error, "") for label, stdin, error in invalid]
    for label, stdin, duration in (
        ("ignored-trailing-input", "1\na 1\n1\na\nignored invalid 0x00\n", 1),
        ("missing-final-newline", "1\na 1\n1\na", 1),
        ("unicode-whitespace-in-count", "\u0085+0_1\u2003\na 2\n1\na\n", 2),
    ):
        expected = oracle((TaskSpec("a", duration),), ("a",))["output"]
        result.append((label, stdin, None, expected))
    return result


def validate_parser_commands(
    commands: dict[str, list[str]], timeout: float = 120
) -> dict:
    checks = []
    for label, stdin, error, expected in parser_cases():
        results = {}
        expected_returncode = 1 if error is not None else 0
        for name, command in commands.items():
            result = subprocess.run(
                command,
                input=stdin.encode(),
                capture_output=True,
                timeout=timeout,
                check=False,
            )
            stderr = result.stderr.decode()
            category = (
                stderr.rstrip().splitlines()[-1].partition(":")[0] if stderr else None
            )
            if (
                result.returncode != expected_returncode
                or result.stdout != expected.encode()
                or category != error
            ):
                raise AssertionError(
                    f"{name}/parser/{label}: expected status {expected_returncode}, "
                    f"error {error!r}, stdout {expected!r}; actual {result!r}"
                )
            results[name] = {
                "returncode": result.returncode,
                "stderr": stderr,
                "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
                "stdout_bytes": len(result.stdout),
            }
        checks.append(
            {
                "case": label,
                "input": stdin,
                "expected_returncode": expected_returncode,
                "expected_error_category": error,
                "expected_stdout": expected,
                "results": results,
            }
        )
    return {
        "parser_cases": len(checks),
        "parser_passed_by_engine": {name: len(checks) for name in commands},
        "parser_policy": "Fourteen malformed inputs require exit 1, empty stdout, and the same final error category; three valid framing variants require the full independent oracle output and no stderr. These cases are separate from graph-oracle counts and are in range for checked i64.",
        "parser_checks": checks,
    }


def validate_commands(commands: dict[str, list[str]], timeout: float = 120) -> dict:
    checks = []
    passed = {name: 0 for name in commands}
    excluded = []
    input_targets = 0
    for case in cases():
        expected = oracle(case.tasks, case.changes)
        stdin = case.stdin_override or input_text(case.tasks, case.changes)
        output = expected["output"].encode()
        reason = i64_exclusion(case.tasks, expected)
        if reason is not None:
            excluded.append({"case": case.label, "reason": reason})
        results = {}
        for name, command in commands.items():
            result = subprocess.run(
                command,
                input=stdin.encode(),
                capture_output=True,
                timeout=timeout,
                check=False,
            )
            rejects = name == "rust_i64" and reason is not None
            if rejects:
                if (
                    result.returncode != 1
                    or result.stdout
                    or not result.stderr.startswith(b"OverflowError:")
                ):
                    raise AssertionError(
                        f"{name}/{case.label}: expected explicit overflow rejection: {result!r}"
                    )
            else:
                if result.returncode != 0 or result.stderr or result.stdout != output:
                    raise AssertionError(
                        f"{name}/{case.label}: complete-output mismatch\nexpected {output!r}\nactual {result!r}"
                    )
                passed[name] += 1
            results[name] = {
                "returncode": result.returncode,
                "stderr": result.stderr.decode(),
                "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
                "stdout_bytes": len(result.stdout),
            }
        checks.append(
            {
                "case": case.label,
                "input": stdin,
                "expected_stdout": expected["output"],
                "kind": expected["kind"],
                "affected": len(expected["order"]),
                "i64_exclusion": reason,
                "results": results,
            }
        )
        input_targets += len(case.tasks)
    return {
        "oracle_cases": len(checks),
        "input_targets": input_targets,
        "passed_by_engine": passed,
        "i64_excluded_cases": excluded,
        "i64_overflow_rejections_checked": len(excluded)
        if "rust_i64" in commands
        else 0,
        "i64_policy": "Out-of-range parsed durations and affected finishes are excluded from semantic equivalence and checked for explicit overflow rejection; graph validation precedes rebuild arithmetic.",
        "oracle": "graphlib topology, fixed-point affected-set closure, heapq min-ID ready selection, independent predecessor maximums; compiled package never imported",
        "checks": checks,
        **validate_parser_commands(commands, timeout),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust-bigint", type=Path)
    parser.add_argument("--rust-i64", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    commands = {"cpython": python_command(args.python)}
    for name, path in (
        ("relar", args.binary),
        ("rust_bigint", args.rust_bigint),
        ("rust_i64", args.rust_i64),
    ):
        if path is not None:
            commands[name] = [str(path.resolve(strict=True))]
    result = validate_commands(commands)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(
        json.dumps(
            {
                key: value
                for key, value in result.items()
                if key not in {"checks", "parser_checks"}
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
