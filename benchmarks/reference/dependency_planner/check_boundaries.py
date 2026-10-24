"""Validate planner implementations against the independent oracle and boundaries."""

import argparse
import graphlib
import hashlib
import json
import runpy
import subprocess
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
dependency = SimpleNamespace(
    **runpy.run_path(str(ROOT / "benchmarks/dependency_benchmark.py"))
)
validator = dependency.validator
shared = dependency.shared
Task = validator.TaskSpec
MAX = (1 << 63) - 1
MIN = -(1 << 63)


def i64_exclusion(tasks: tuple) -> str | None:
    """Exclude unrepresentable inputs or overflow on any schedulable task."""
    for task_id, task in enumerate(tasks):
        if not MIN <= task.duration <= MAX:
            return f"task {task_id} duration {task.duration} is outside signed 64-bit range"
        for value in task.dependencies:
            if not MIN <= value <= MAX:
                return (
                    f"task {task_id} dependency {value} is outside signed 64-bit range"
                )
    # Graph validation finishes before scheduling, so these errors cannot cause
    # an accumulated finish overflow. Parsing still requires every value to fit.
    if any(
        task.duration < 0
        or any(not 0 <= value < len(tasks) for value in task.dependencies)
        for task in tasks
    ):
        return None
    graph = {i: set(task.dependencies) for i, task in enumerate(tasks)}
    sorter = graphlib.TopologicalSorter(graph)
    try:
        sorter.prepare()
    except graphlib.CycleError:
        pass  # A cyclic graph can still contain schedulable acyclic components.
    finishes = [0] * len(tasks)
    while ready := sorter.get_ready():
        for task_id in ready:
            finish = (
                max((finishes[i] for i in graph[task_id]), default=0)
                + tasks[task_id].duration
            )
            if finish > MAX:
                return f"task {task_id} earliest finish {finish} exceeds signed 64-bit range"
            finishes[task_id] = finish
            sorter.done(task_id)
    return None


def validate_oracle(commands: dict) -> dict:
    """Run the shared independent oracle once per case and check every engine."""
    counts = {name: 0 for name in commands}
    excluded = []
    cases = validator.cases()
    for case in cases:
        stdin = (
            case.stdin_override
            if case.stdin_override is not None
            else validator.input_text(case.tasks, case.lookup)
        )
        expected = validator.expected_output(case.tasks, case.lookup)
        exclusion = i64_exclusion(case.tasks)
        if exclusion is not None:
            excluded.append({"case": case.label, "reason": exclusion})
        for name, command in commands.items():
            if name == "rust_i64" and exclusion is not None:
                continue
            actual = validator.execute(command, stdin)
            if actual != expected:
                raise AssertionError(
                    f"{name}/oracle mismatch in {case.label}: {actual!r} != {expected!r}"
                )
            counts[name] += 1
    return {
        "oracle_cases": len(cases),
        "input_tasks": sum(len(case.tasks) for case in cases),
        "passed_by_engine": counts,
        "i64_excluded_cases": excluded,
        "i64_policy": "exclude any out-of-range parsed duration/dependency or earliest finish exceeding signed 64-bit range, including schedulable components of cyclic graphs",
        "oracle": "shared dependency_validate.py: graphlib cycle detection, ready-set order, independent dependency maximums, and complete report comparison",
    }


CASES = [
    ("i64-max", (Task("a", MAX),), "a", False, None),
    ("i64-min-negative-duration", (Task("a", MIN),), "a", False, None),
    ("i64-max-plus-one-input", (Task("a", MAX + 1),), "a", True, None),
    ("i64-min-minus-one-input", (Task("a", MIN - 1),), "a", True, None),
    ("finish-at-i64-max", (Task("a", MAX - 1), Task("b", 1, (0,))), "b", False, None),
    ("finish-overflows-i64", (Task("a", MAX), Task("b", 1, (0,))), "b", True, None),
    (
        "overflow-before-cycle",
        (Task("a", MAX), Task("b", 1, (0,)), Task("c", 1, (2,))),
        "b",
        True,
        None,
    ),
    (
        "graph-error-precedes-overflow",
        (Task("a", MAX), Task("b", 1, (0,)), Task("c", 2, (3,))),
        "b",
        False,
        None,
    ),
    (
        "separators-and-signs",
        (Task("x", MAX - 1), Task("y", 1, (0,))),
        "y",
        False,
        "2\nx +9_223_372_036_854_775_806\ny +1 +0\n y\n",
    ),
    ("embedded-nul-label", (Task("a\x00x", 2),), "a\x00x", False, None),
]


def protect_outputs(outputs: list[Path], inputs: list[Path]) -> None:
    """Reject output collisions, including symlinks and existing hardlinks."""
    protected = list(inputs)
    for output in outputs:
        if any(
            output.resolve() == path.resolve()
            or (output.exists() and path.exists() and output.samefile(path))
            for path in protected
        ):
            raise ValueError("outputs must be distinct and must not overwrite inputs")
        protected.append(output)


def validate(commands: dict, timeout: float = 120) -> dict:
    """Check normal reports and expected bounded-integer failures without timing."""
    checks = []
    for label, tasks, lookup, rejects_i64, override in CASES:
        stdin = (
            override if override is not None else validator.input_text(tasks, lookup)
        )
        expected = validator.expected_output(tasks, lookup).encode()
        results = {}
        for engine, command in commands.items():
            result = subprocess.run(
                command,
                input=stdin.encode(),
                capture_output=True,
                timeout=timeout,
                check=False,
            )
            if engine == "rust_i64" and rejects_i64:
                valid = (
                    result.returncode == 1
                    and result.stderr.startswith(b"OverflowError:")
                    and not result.stdout
                )
            else:
                valid = (
                    result.returncode == 0
                    and not result.stderr
                    and result.stdout == expected
                )
            if not valid:
                raise AssertionError(
                    f"{label}/{engine}: unexpected boundary result {result!r}"
                )
            results[engine] = {
                "returncode": result.returncode,
                "stderr": result.stderr.decode(),
                "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
                "stdout_bytes": len(result.stdout),
            }
        checks.append(
            {
                "case": label,
                "input": stdin,
                "expected_stdout": expected.decode(),
                "i64_rejects": rejects_i64,
                "results": results,
            }
        )
    return {
        "cases": len(checks),
        "checks": checks,
        "i64_overflow_rejections": sum(case[3] for case in CASES),
        "binary_sha256": {
            name: shared.sha256(Path(shared.executable(command[0])))
            for name, command in commands.items()
        },
        "harness_sha256": shared.sha256(Path(__file__)),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--rust-bigint", type=Path, required=True)
    parser.add_argument("--rust-i64", type=Path, required=True)
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "benchmarks/local/dependency-reference/boundary-validation.json",
    )
    args = parser.parse_args()
    commands = {"cpython": validator.python_command(shared.executable(args.python))}
    for name in ("baseline", "imre", "rust_bigint", "rust_i64"):
        path = getattr(args, name)
        if path is not None:
            commands[name] = [shared.executable(str(path))]
    source_paths = [
        *validator.package_sources(),
        ROOT / "benchmarks/dependency_validate.py",
        ROOT / "benchmarks/dependency_benchmark.py",
        ROOT / "benchmarks/benchmark.py",
        ROOT / "benchmarks/validate.py",
        HERE / "Cargo.toml",
        HERE / "Cargo.lock",
        HERE / "build.py",
        Path(__file__).resolve(),
        *sorted((HERE / "src").rglob("*.rs")),
    ]
    inputs = {
        **{
            f"executable:{name}": Path(command[0]).resolve()
            for name, command in commands.items()
        },
        **{str(path.relative_to(ROOT)): path for path in source_paths},
    }
    protect_outputs([args.output], list(inputs.values()))
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    oracle = validate_oracle(commands)
    record = validate(commands, args.timeout)
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"{name} changed during validation")
    record.update(validation=oracle, input_sha256=hashes)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n")
    print(f"Oracle cases passed: {oracle['passed_by_engine']}")
    print(f"All {len(CASES)} boundary cases passed; four cases reject i64 overflow.")


if __name__ == "__main__":
    main()
