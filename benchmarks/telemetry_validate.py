"""Check telemetry queries, arbitrary integers, errors, and complete file effects."""

import argparse
import json
import os
import posixpath
import random
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "telemetry_comparison"
ENTRY = PACKAGE / "__main__.py"
OPERATIONS = ("report", "alert", "within", "latest")
files = SimpleNamespace(**runpy.run_path(str(ROOT / "benchmarks/manifest_validate.py")))
Case = files.Case
digest = files.digest
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
snapshot = files.snapshot
encoded_files = files.encoded_files
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['telemetry_comparison', *sys.argv[2:]]; "
    "runpy.run_module('telemetry_comparison', run_name='__main__', alter_sys=True)"
)


def python_command(python: str) -> list[str]:
    return [python, "-I", "-B", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def inputs(
    reference: list[tuple[int, int]], observed: list[tuple[int, int]]
) -> dict[str, bytes]:
    return {
        name: (
            "\n".join([str(len(rows)), *(f"{stamp} {value}" for stamp, value in rows)])
            + "\n"
        ).encode()
        for name, rows in (("reference.txt", reference), ("observed.txt", observed))
    }


def case(
    label: str, reference, observed, operation="report", threshold=3, repetitions=2
) -> Case:
    return Case(
        label,
        inputs(reference, observed),
        (
            operation,
            "reference.txt",
            "observed.txt",
            "report.txt",
            str(threshold),
            str(repetitions),
        ),
    )


def number(token: str, label: str) -> int:
    """Independent regex parser for the explicitly bounded decimal protocol."""
    if re.fullmatch(r"[+-]?[0-9]{1,4300}", token) is None:
        raise ValueError(label + ": invalid integer")
    return int(token)


def parse(text: str, label: str) -> list[tuple[int, int]]:
    rows = text.splitlines()
    if not rows:
        raise ValueError(label + ": missing count")
    first = rows[0].split()
    if len(first) != 1:
        raise ValueError(label + ": expected count")
    count = number(first[0], label)
    if count < 0:
        raise ValueError(label + ": negative count")
    result = []
    for position in range(count):
        if position + 1 >= len(rows):
            raise ValueError(label + ": truncated")
        fields = rows[position + 1].split()
        if len(fields) != 2:
            raise ValueError(label + ": expected timestamp and value")
        timestamp, value = (number(field, label) for field in fields)
        if result and timestamp <= result[-1][0]:
            raise ValueError(label + ": timestamps must increase")
        result.append((timestamp, value))
    if count + 1 != len(rows):
        raise ValueError(label + ": trailing data")
    return result


def query(operation: str, reference, observed, threshold: int) -> tuple[bytes, int]:
    """Use explicit indexed alignment and a materialized independent error model."""
    for position in range(min(len(reference), len(observed))):
        if reference[position][0] != observed[position][0]:
            raise ValueError("traces: timestamps differ")
    if len(reference) > len(observed):
        raise ValueError("zip() argument 2 is shorter than argument 1")
    if len(reference) < len(observed):
        raise ValueError("zip() argument 2 is longer than argument 1")
    if operation not in OPERATIONS:
        raise ValueError("unknown telemetry operation")
    errors = [observed[i][1] - reference[i][1] for i in range(len(reference))]
    alerts = [
        reference[i][0] for i, error in enumerate(errors) if abs(error) > threshold
    ]
    latest = max(alerts) if alerts else None
    has_alert = bool(alerts)
    if operation == "alert":
        return f"{has_alert}\n".encode(), int(has_alert)
    if operation == "within":
        return f"{not has_alert}\n".encode(), int(not has_alert)
    if operation == "latest":
        return f"{latest}\n".encode(), latest if latest is not None else 0
    signed = sum(errors)
    absolute = sum(map(abs, errors))
    squared = sum(error**2 for error in errors)
    report = (
        f"TELEMETRY\nSAMPLES {len(reference)}\nSIGNED_ERROR {signed}\n"
        f"ABSOLUTE_ERROR {absolute}\nSQUARED_ERROR {squared}\n"
        f"ALERT {has_alert}\nWITHIN {not has_alert}\nLATEST {latest}\nEND\n"
    )
    return report.encode(), signed + absolute + squared


def oracle(value: Case) -> dict:
    nodes = {name: [data] for name, data in value.files.items()}
    for name, target in value.hardlinks.items():
        nodes[name] = nodes[target]
    directories = files.directory_names(value)

    def resolve(path: str) -> str:
        if "\0" in path:
            raise ValueError
        if not path:
            raise OSError
        prefix = []
        for component in path.split("/")[:-1]:
            if component == "..":
                if not prefix:
                    raise OSError
                prefix.pop()
            elif component not in ("", "."):
                prefix.append(component)
            if ("/".join(prefix) or ".") not in directories:
                raise OSError
        name = posixpath.normpath(path)
        parent = posixpath.dirname(name)
        if name in directories or (parent and parent not in directories):
            raise OSError
        return name

    def load(path: str, label: str) -> str:
        try:
            name = resolve(path)
            if name not in nodes:
                raise OSError
            return nodes[name][0].decode()
        except UnicodeDecodeError:
            raise ValueError(label + ": invalid UTF-8") from None
        except OSError:
            raise OSError(label + ": cannot read") from None
        except ValueError:
            raise ValueError(label + ": invalid path") from None

    try:
        if len(value.arguments) != 6:
            raise ValueError(
                "usage: telemetry_comparison MODE REFERENCE OBSERVED REPORT THRESHOLD REPEATS"
            )
        operation, reference_path, observed_path, output_path, limit, repeats = (
            value.arguments
        )
        threshold = number(limit, "threshold")
        repetitions = number(repeats, "repetitions")
        if threshold < 0:
            raise ValueError("threshold must be nonnegative")
        if repetitions < 1:
            raise ValueError("repetitions must be positive")
        reference = parse(load(reference_path, "reference"), "reference")
        observed = parse(load(observed_path, "observed"), "observed")
        output, contribution = query(operation, reference, observed, threshold)
        stdout = f"OK\n{contribution * repetitions}\n"
        try:
            destination = resolve(output_path)
            if destination not in nodes:
                nodes[destination] = [b""]
            nodes[destination][0] = output
        except OSError:
            raise OSError("report: cannot write") from None
        except ValueError:
            raise ValueError("report: invalid path") from None
    except (OSError, ValueError) as error:
        stdout = f"ERROR\n{type(error).__name__}\n{error}\n"
    return {
        "stdout": stdout,
        "files": {name: node[0] for name, node in nodes.items()},
        "directories": sorted(directories - {"."}),
    }


def check_result(value: Case, expected: dict, process, directory: Path) -> dict:
    actual, directories = snapshot(directory)
    assert process.returncode == 0 and not process.stderr, (value.label, process)
    assert process.stdout.decode() == expected["stdout"], (
        value.label,
        process.stdout,
        expected["stdout"],
    )
    assert actual == expected["files"] and directories == expected["directories"], (
        value.label,
        "file effects differ",
    )
    return {
        "returncode": 0,
        "stdout": process.stdout.decode(),
        "stderr": "",
        "files": {
            name: {"sha256": digest(data), "bytes": len(data)}
            for name, data in sorted(actual.items())
        },
        "directories": directories,
    }


def cases() -> list[Case]:
    result = []
    datasets = [
        ("empty", [], [], 0),
        ("one-equal", [(0, 1)], [(0, 1)], 0),
        ("threshold-equal", [(-1, 5), (0, 0), (1, -8)], [(-1, 8), (0, -3), (1, -5)], 3),
        (
            "mixed",
            [(-10, 5), (-3, -4), (0, 1), (9, 8)],
            [(-10, 12), (-3, -4), (0, -6), (9, 10)],
            3,
        ),
        ("negative-latest", [(-9, 0), (-5, 0)], [(-9, 0), (-5, 4)], 3),
        (
            "huge",
            [(-(10**100), 10**150), (10**100, -(10**150))],
            [(-(10**100), -(10**150)), (10**100, 10**150 + 1)],
            10**140,
        ),
        ("native-square-overflow", [(0, -(2**63))], [(0, 2**63 - 1)], 2**63),
        (
            "native-sum-overflow",
            [(i, -(2**62)) for i in range(3)],
            [(i, 2**62) for i in range(3)],
            0,
        ),
        (
            "tag-boundaries",
            [
                (i, value)
                for i, value in enumerate([2**62 - 1, 2**62, 2**63, -(2**63) - 1])
            ],
            [
                (i, value + 1)
                for i, value in enumerate([2**62 - 1, 2**62, 2**63, -(2**63) - 1])
            ],
            1,
        ),
    ]
    generator = random.Random(20261020)
    for index in range(32):
        size = generator.randrange(0, 40)
        stamps = sorted(generator.sample(range(-1000, 1000), size))
        scale = 10**80 if index % 8 == 0 else 1
        reference = [
            (stamp, generator.randrange(-100, 100) * scale) for stamp in stamps
        ]
        observed = [
            (stamp, value + generator.randrange(-10, 11) * scale)
            for stamp, value in reference
        ]
        datasets.append(
            (f"random-{index}", reference, observed, generator.randrange(11) * scale)
        )
    for label, reference, observed, threshold in datasets:
        for operation in OPERATIONS:
            result.append(
                case(
                    f"{label}-{operation}", reference, observed, operation, threshold, 3
                )
            )
    for operation in OPERATIONS:
        result.extend(
            [
                case(f"shorter-{operation}", [(0, 0), (1, 0)], [(0, 99)], operation),
                case(f"longer-{operation}", [(0, 0)], [(0, 99), (1, 0)], operation),
                case(
                    f"timestamp-{operation}",
                    [(0, 0), (1, 0)],
                    [(0, 99), (2, 0)],
                    operation,
                ),
            ]
        )
    result.extend(
        [
            case(
                "squared-decimal-output-limit",
                [(0, 0)],
                [(0, 10**2151)],
                "report",
                0,
                1,
            ),
            case(
                "checksum-decimal-output-limit",
                [(9 * 10**4299, 0)],
                [(9 * 10**4299, 1)],
                "latest",
                0,
                2,
            ),
        ]
    )
    valid = case("valid", [(0, 5), (1, 7)], [(0, 5), (1, 11)])
    invalid = [
        ("empty", b""),
        ("missing-count", b"\n"),
        ("count-fields", b"1 2\n"),
        ("negative-count", b"-1\n"),
        ("bad-count", b"x\n"),
        ("truncated", b"1\n"),
        ("huge-count", (str(10**100) + "\n").encode()),
        ("trailing", b"0\nextra\n"),
        ("trailing-blank", b"0\n\n"),
        ("row-fields", b"1\n0\n"),
        ("extra-row-field", b"1\n0 1 2\n"),
        ("bad-value", b"1\n0 0x10\n"),
        ("underscore", b"1\n0 1_0\n"),
        ("double-sign", b"1\n0 +-1\n"),
        ("sign-only", b"1\n0 -\n"),
        ("non-ascii-digit", "1\n0 ١\n".encode()),
        ("invalid-utf8", b"\xff\xfe"),
        ("nul", b"1\n0 1\0\n"),
        ("duplicate-time", b"2\n0 1\n0 2\n"),
        ("decreasing-time", b"2\n1 1\n0 2\n"),
        ("too-many-digits", ("1\n0 " + "1" * 4301 + "\n").encode()),
    ]
    for label, payload in invalid:
        for source in ("reference", "observed"):
            result.append(
                Case(
                    f"{source}-{label}",
                    {**valid.files, source + ".txt": payload, "report.txt": b"keep"},
                    valid.arguments,
                )
            )
    for position, token in ((4, "-1"), (4, "x"), (5, "0"), (5, "-1"), (5, "x")):
        arguments = list(valid.arguments)
        arguments[position] = token
        result.append(
            Case(
                f"argument-{position}-{token}",
                {**valid.files, "report.txt": b"keep"},
                tuple(arguments),
            )
        )
    for separator in (
        "\n",
        "\r\n",
        "\r",
        "\v",
        "\f",
        "\x1c",
        "\x1d",
        "\x1e",
        "\x85",
        "\u2028",
        "\u2029",
    ):
        result.append(
            Case(
                "lines-" + separator.encode().hex(),
                {
                    name: data.replace(b"\n", separator.encode())
                    for name, data in valid.files.items()
                },
                valid.arguments,
            )
        )
    result.extend(
        [
            Case(
                "whitespace-signs",
                {
                    "reference.txt": "\u2003+1\u2003\n-0\x1f+4\n".encode(),
                    "observed.txt": b"1\n0 -4\n",
                },
                valid.arguments,
            ),
            Case(
                "no-final-newline",
                {name: data.rstrip(b"\n") for name, data in valid.files.items()},
                valid.arguments,
            ),
            Case(
                "overwrite-report",
                {**valid.files, "report.txt": b"replace"},
                valid.arguments,
            ),
            Case(
                "report-is-reference",
                valid.files,
                (*valid.arguments[:3], "reference.txt", *valid.arguments[4:]),
            ),
            Case(
                "report-is-observed",
                valid.files,
                (*valid.arguments[:3], "observed.txt", *valid.arguments[4:]),
            ),
            Case(
                "hardlink-report",
                valid.files,
                valid.arguments,
                hardlinks={"report.txt": "reference.txt"},
            ),
            Case(
                "same-input-report",
                {"both.txt": b"0\n"},
                ("report", "both.txt", "both.txt", "both.txt", "0", "1"),
            ),
            Case(
                "missing-reference",
                {"observed.txt": valid.files["observed.txt"], "report.txt": b"keep"},
                valid.arguments,
            ),
            Case(
                "missing-observed",
                {"reference.txt": valid.files["reference.txt"], "report.txt": b"keep"},
                valid.arguments,
            ),
            Case(
                "reference-directory",
                {"observed.txt": valid.files["observed.txt"]},
                valid.arguments,
                ("reference.txt",),
            ),
            Case(
                "observed-directory",
                {"reference.txt": valid.files["reference.txt"]},
                valid.arguments,
                ("observed.txt",),
            ),
            Case("report-directory", valid.files, valid.arguments, ("report.txt",)),
            Case(
                "report-missing-parent",
                valid.files,
                (*valid.arguments[:3], "absent/report.txt", *valid.arguments[4:]),
            ),
            Case(
                "report-file-parent",
                {**valid.files, "parent": b"file"},
                (*valid.arguments[:3], "parent/report.txt", *valid.arguments[4:]),
            ),
            Case(
                "empty-report-path",
                valid.files,
                (*valid.arguments[:3], "", *valid.arguments[4:]),
            ),
            Case(
                "reference-parse-before-observed-open",
                {"reference.txt": b"x\n"},
                valid.arguments,
            ),
            Case(
                "observed-parse-before-report-open",
                {**valid.files, "observed.txt": b"x\n"},
                valid.arguments,
                ("report.txt",),
            ),
            Case(
                "relative-path",
                valid.files,
                (
                    "report",
                    "folder/../reference.txt",
                    "observed.txt",
                    "folder/report.txt",
                    "3",
                    "2",
                ),
                ("folder",),
            ),
            Case(
                "unicode-path",
                {
                    "ref café.txt": valid.files["reference.txt"],
                    "観測.txt": valid.files["observed.txt"],
                },
                ("report", "ref café.txt", "観測.txt", "結果.txt", "3", "2"),
            ),
            Case("unknown-operation", valid.files, ("unknown", *valid.arguments[1:])),
            Case("too-few-arguments", valid.files, valid.arguments[:-1]),
            Case("too-many-arguments", valid.files, (*valid.arguments, "extra")),
        ]
    )
    assert len({value.label for value in result}) == len(result)
    return result


def validate_commands(commands: dict[str, list[str]], timeout: float = 30) -> dict:
    records = []
    for value in cases():
        expected = oracle(value)
        engines = {}
        for name, command in commands.items():
            with tempfile.TemporaryDirectory(prefix="telemetry-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run(
                    [*command, *value.arguments],
                    input=b"",
                    capture_output=True,
                    cwd=directory,
                    timeout=timeout,
                    check=False,
                    env={
                        **os.environ,
                        "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1",
                    },
                )
                engines[name] = check_result(value, expected, process, directory)
        records.append(
            {
                "case": case_record(value),
                "expected_stdout": expected["stdout"],
                "expected_files": encoded_files(expected["files"]),
                "engines": engines,
            }
        )
    device = {}
    if Path("/dev/full").exists():
        value = case("terminal-output-device-error", [(0, 0)], [(0, 4)])
        expected = {
            "stdout": "ERROR\nOSError\nreport: cannot write\n",
            "files": value.files,
            "directories": [],
        }
        for name, command in commands.items():
            with tempfile.TemporaryDirectory(prefix="telemetry-device-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                arguments = (*value.arguments[:3], "/dev/full", *value.arguments[4:])
                process = subprocess.run(
                    [*command, *arguments],
                    input=b"",
                    capture_output=True,
                    cwd=directory,
                    timeout=timeout,
                    check=False,
                    env={
                        **os.environ,
                        "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1",
                    },
                )
                device[name] = check_result(value, expected, process, directory)
    return {
        "cases": len(records),
        "engines": list(commands),
        "records": records,
        "output_device": device,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    commands = {"cpython": python_command(args.python)}
    paths = {
        "validator": Path(__file__).resolve(),
        "filesystem_validator": ROOT / "benchmarks/manifest_validate.py",
        "batch_validator": ROOT / "benchmarks/batch_validate.py",
        "incremental_validator": ROOT / "benchmarks/incremental_validate.py",
        "python": Path(shutil.which(args.python) or args.python).resolve(strict=True),
    }
    paths.update(
        {
            "example:" + str(path.relative_to(PACKAGE)): path
            for path in sorted(PACKAGE.rglob("*.py"))
        }
    )
    for name, binary in (("imre", args.binary), ("rust", args.rust)):
        if binary is not None:
            paths[name] = binary.resolve(strict=True)
            commands[name] = [str(paths[name])]
    hashes = {name: digest(path.read_bytes()) for name, path in paths.items()}
    result = validate_commands(commands, args.timeout)
    assert hashes == {name: digest(path.read_bytes()) for name, path in paths.items()}
    result.update(
        {"schema": 1, "source_and_binary_sha256": hashes, "input_guards_passed": True}
    )
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(
        json.dumps(
            {key: value for key, value in result.items() if key != "records"}, indent=2
        )
    )


if __name__ == "__main__":
    main()
