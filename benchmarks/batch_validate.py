"""Independent framing, recovery, and graph oracle for the batch planner."""

import argparse
import hashlib
import json
import runpy
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "batch_planner"
ENTRY = PACKAGE / "__main__.py"
incremental = SimpleNamespace(
    **runpy.run_path(str(ROOT / "benchmarks/incremental_validate.py"))
)
BOOTSTRAP = (
    "import runpy, sys; "
    "sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['batch_planner']; "
    "runpy.run_module('batch_planner', run_name='__main__', alter_sys=True)"
)


@dataclass(frozen=True)
class Case:
    label: str
    stdin: str


class PayloadError(Exception):
    def __init__(self, kind: str, message: str) -> None:
        super().__init__(message)
        self.kind = kind
        self.message = message


def package_sources() -> list[Path]:
    return sorted((*PACKAGE.rglob("*.py"), *incremental.package_sources()))


def python_command(python: str) -> list[str]:
    return [python, "-I", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def payload_lines(text: str) -> tuple[str, ...]:
    if not text:
        return ()
    return tuple(text.removesuffix("\n").split("\n"))


def frame(payloads: tuple[tuple[str, ...], ...]) -> str:
    rows = [str(len(payloads))]
    for payload in payloads:
        assert all("\n" not in line for line in payload)
        rows.append(str(len(payload)))
        rows.extend(payload)
    return "\n".join(rows) + "\n"


def parse_payload(lines: tuple[str, ...]) -> tuple[tuple, tuple[str, ...]]:
    """Use an iterator and explicit grammar checks, without importing the package."""
    remaining = iter(lines)

    def take() -> str:
        try:
            return next(remaining)
        except StopIteration:
            raise PayloadError("IndexError", "truncated payload") from None

    def integer(text: str) -> int:
        try:
            return int(text)
        except ValueError:
            raise PayloadError("ValueError", "invalid integer") from None

    count = integer(take())
    if count < 0:
        raise PayloadError("ValueError", "negative target count")
    tasks = []
    for _ in range(count):
        words = take().split()
        if len(words) < 2:
            raise PayloadError("AssertionError", "target row needs a name and duration")
        tasks.append(
            incremental.TaskSpec(words[0], integer(words[1]), tuple(words[2:]))
        )
    changed_count = integer(take())
    if changed_count < 0:
        raise PayloadError("ValueError", "negative change count")
    changes = []
    for _ in range(changed_count):
        words = take().split()
        if len(words) != 1:
            raise PayloadError("AssertionError", "change row needs exactly one name")
        changes.append(words[0])
    return tuple(tasks), tuple(changes)


def oracle(stdin: str) -> dict:
    """Frame first, recover only inside a complete frame, then use graphlib's oracle."""
    remaining = iter(payload_lines(stdin))
    output = []
    requests = []
    completed = 0
    invalid = 0

    def take() -> str:
        try:
            return next(remaining)
        except StopIteration:
            raise EOFError from None

    try:
        count = int(take())
        if count < 0:
            raise AssertionError
        for request_id in range(count):
            line_count = int(take())
            if line_count < 0:
                raise AssertionError
            # A partial frame produces no REQUEST header and cannot be recovered.
            lines = tuple(take() for _ in range(line_count))
            output.append(f"REQUEST\n{request_id}\n")
            try:
                tasks, changes = parse_payload(lines)
            except PayloadError as error:
                output.append(f"INVALID\n{error.kind}\n{error.message}\n")
                invalid += 1
                requests.append(
                    {"id": request_id, "kind": "invalid", "error": error.kind}
                )
            else:
                result = incremental.oracle(tasks, changes)
                output.append(result["output"])
                completed += 1
                requests.append(
                    {
                        "id": request_id,
                        "kind": result["kind"],
                        "targets": len(tasks),
                        "affected": len(result["order"]),
                    }
                )
            output.append(f"END\n{request_id}\n")
    except (EOFError, ValueError, AssertionError) as error:
        category = type(error).__name__
        returncode = 1
    else:
        category = None
        returncode = 0
        output.append(
            f"SUMMARY\nCOMPLETED\n{completed}\nINVALID\n{invalid}\nCLEANED\n{len(requests)}\n"
        )
    return {
        "returncode": returncode,
        "error_category": category,
        "stdout": "".join(output),
        "requests": requests,
        "completed": completed,
        "invalid": invalid,
        "cleaned": len(requests),
    }


def cases() -> list[Case]:
    result = [Case("empty-batch", frame(()))]
    graph_payloads = []
    for case in incremental.cases():
        text = case.stdin_override or incremental.input_text(case.tasks, case.changes)
        payload = payload_lines(text)
        graph_payloads.append(payload)
        result.append(Case("graph-" + case.label, frame((payload,))))
    valid = ("1", "recovered 7", "1", "recovered")
    for label, text, _, _ in incremental.parser_cases():
        result.append(Case("recover-" + label, frame((payload_lines(text), valid))))
    malformed = ((), ("1", "bad"), ("1", "bad x", "0"), ("-1",))
    result.extend(
        (
            Case("all-graph-cases-in-one-batch", frame(tuple(graph_payloads))),
            Case("repeated-errors-then-valid", frame(malformed * 8 + (valid,))),
            Case(
                "failure-after-partial-allocation",
                frame(
                    (
                        ("25", *(f"task_{i} 1" for i in range(24)), "bad x", "0"),
                        valid,
                    )
                ),
            ),
            Case(
                "markers-are-ordinary-names",
                frame((("2", "REQUEST 2", "END 3 REQUEST", "1", "REQUEST"),)),
            ),
            Case("payload-extra-lines-ignored", frame(((*valid, "bad extra data"),))),
            Case(
                "crlf-frames", frame((valid, malformed[1], valid)).replace("\n", "\r\n")
            ),
            Case("missing-final-newline", frame((valid,)).removesuffix("\n")),
            Case("outer-extra-lines-ignored", "0\nignored\n"),
            Case("unicode-framing-count", "\u0085+0_1\u2003\n4\n1\na 2\n1\na\n"),
            Case("huge-target-count-truncated", frame(((str(10**100),), valid))),
            Case(
                "huge-target-count-earlier-integer-error",
                frame(((str(10**100), "bad x"), valid)),
            ),
            Case(
                "huge-change-count-truncated",
                frame((("1", "a 1", str(10**100), "a"), valid)),
            ),
            Case("framing-empty", ""),
            Case("framing-negative-request-count", "-1\n"),
            Case("framing-invalid-request-count", "invalid\n"),
            Case("framing-missing-line-count", "1\n"),
            Case("framing-negative-line-count", "1\n-1\n"),
            Case("framing-invalid-line-count", "1\ninvalid\n"),
            Case("framing-truncated-payload", "1\n2\n0\n"),
            Case("framing-failure-after-valid", "2\n4\n1\na 1\n1\na\n2\n0\n"),
            Case("framing-split-only-whitespace", "\x1c1\n0\n"),
        )
    )
    return result


def validate_commands(commands: dict[str, list[str]], timeout: float = 120) -> dict:
    checks = []
    for case in cases():
        expected = oracle(case.stdin)
        results = {}
        output = expected["stdout"].encode()
        for name, command in commands.items():
            run = subprocess.run(
                command,
                input=case.stdin.encode(),
                capture_output=True,
                timeout=timeout,
                check=False,
            )
            stderr = run.stderr.decode()
            lines = stderr.rstrip().splitlines()
            category = lines[-1].partition(":")[0] if lines else None
            if (
                run.returncode != expected["returncode"]
                or run.stdout != output
                or category != expected["error_category"]
                or (expected["returncode"] == 0 and stderr)
            ):
                raise AssertionError(
                    f"{name}/{case.label}: expected {expected!r}; actual {run!r}"
                )
            results[name] = {
                "returncode": run.returncode,
                "stderr": stderr,
                "stdout_sha256": hashlib.sha256(run.stdout).hexdigest(),
                "stdout_bytes": len(run.stdout),
            }
        checks.append(
            {
                "case": case.label,
                "input": case.stdin,
                "expected": expected,
                "results": results,
            }
        )
    return {
        "cases": len(checks),
        "passed_by_engine": {name: len(checks) for name in commands},
        "requests": sum(len(check["expected"]["requests"]) for check in checks),
        "completed_requests": sum(check["expected"]["completed"] for check in checks),
        "invalid_requests": sum(check["expected"]["invalid"] for check in checks),
        "fatal_framing_cases": sum(
            check["expected"]["returncode"] != 0 for check in checks
        ),
        "oracle": "Independent iterator-based framing and payload grammar; existing independent graphlib/fixed-point/heapq graph oracle; compiled packages never imported",
        "checks": checks,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust-bigint", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    commands = {"cpython": python_command(args.python)}
    for name, path in (("relar", args.binary), ("rust_bigint", args.rust_bigint)):
        if path is not None:
            commands[name] = [str(path.resolve(strict=True))]
    result = validate_commands(commands)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(
        json.dumps(
            {key: value for key, value in result.items() if key != "checks"}, indent=2
        )
    )


if __name__ == "__main__":
    main()
