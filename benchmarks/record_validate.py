"""Independent payout arithmetic and complete filesystem oracle."""

import argparse
import json
import posixpath
import random
import re
import runpy
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "record_pipeline"
ENTRY = PACKAGE / "__main__.py"
files = SimpleNamespace(**runpy.run_path(str(HERE / "manifest_validate.py")))
Case = files.Case
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
encoded_files = files.encoded_files
digest = files.digest
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['record_pipeline', *sys.argv[2:]]; "
    "runpy.run_module('record_pipeline', run_name='__main__', alter_sys=True)"
)


def python_command(python: str) -> list[str]:
    return [python, "-I", "-B", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def case(label, rows, mode="net", limit=10, fee=25, repetitions=2):
    text = "\n".join([str(len(rows)), *(f"{i} {amount} {risk}" for i, amount, risk in rows)]) + "\n"
    return Case(label, {"records.txt": text.encode()},
                (mode, "records.txt", "report.txt", str(limit), str(fee), str(repetitions)))


def number(token: str, label: str) -> int:
    if re.fullmatch(r"[+-]?[0-9]{1,4300}", token) is None:
        raise ValueError(label + ": invalid integer")
    return int(token)


def parse(text: str) -> list[tuple[int, int, int]]:
    rows = text.splitlines()
    if not rows:
        raise ValueError("records: missing count")
    first = rows[0].split()
    if len(first) != 1:
        raise ValueError("records: expected count")
    count = number(first[0], "records")
    if count < 0:
        raise ValueError("records: negative count")
    result = []
    for position in range(count):
        if position + 1 >= len(rows):
            raise ValueError("records: truncated")
        fields = rows[position + 1].split()
        if len(fields) != 3:
            raise ValueError("records: expected id, amount and risk")
        identifier, amount, risk = [number(field, "records") for field in fields]
        if identifier < 1:
            raise ValueError("records: id must be positive")
        if result and identifier <= result[-1][0]:
            raise ValueError("records: ids must increase")
        if risk < 0:
            raise ValueError("records: risk must be nonnegative")
        result.append((identifier, amount, risk))
    if count + 1 != len(rows):
        raise ValueError("records: trailing data")
    return result


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

    try:
        if len(value.arguments) != 6:
            raise ValueError("usage: record_pipeline MODE INPUT REPORT LIMIT FEE REPEATS")
        mode, input_path, output_path, limit_text, fee_text, repeat_text = value.arguments
        limit = number(limit_text, "limit")
        fee = number(fee_text, "fee")
        repetitions = number(repeat_text, "repetitions")
        if limit < 0 or fee < 0:
            raise ValueError("limit and fee must be nonnegative")
        if repetitions < 1:
            raise ValueError("repetitions must be positive")
        if mode not in ("gross", "net"):
            raise ValueError("unknown payout mode")
        try:
            name = resolve(input_path)
            if name not in nodes:
                raise OSError
            text = nodes[name][0].decode()
        except UnicodeDecodeError:
            raise ValueError("records: invalid UTF-8") from None
        except OSError:
            raise OSError("records: cannot read") from None
        except ValueError:
            raise ValueError("records: invalid path") from None
        rows = parse(text)
        # Materialized comprehensions deliberately do not reproduce the callback
        # pipeline: they independently specify the final accepted payout set.
        adjusted = [amount - (fee if mode == "net" else 0)
                    for _, amount, risk in rows if risk <= limit]
        payouts = [amount for amount in adjusted if amount > 0]
        count, total, largest = len(payouts), sum(payouts), max(payouts, default=0)
        report = f"PAYOUTS\nCOUNT {count}\nTOTAL {total}\nLARGEST {largest}\nEND\n".encode()
        stdout = f"OK\n{(count + total + largest) * repetitions}\n"
        try:
            destination = resolve(output_path)
            if destination not in nodes:
                nodes[destination] = [b""]
            nodes[destination][0] = report
        except OSError:
            raise OSError("report: cannot write") from None
        except ValueError:
            raise ValueError("report: invalid path") from None
    except (OSError, ValueError) as error:
        stdout = f"ERROR\n{type(error).__name__}\n{error}\n"
    return {"stdout": stdout, "files": {name: node[0] for name, node in nodes.items()},
            "directories": sorted(directories - {"."})}


def check_result(value, expected, process, directory):
    actual, directories = files.snapshot(directory)
    assert process.returncode == 0 and process.stderr == b"", (value.label, process)
    assert process.stdout.decode() == expected["stdout"], (value.label, process.stdout, expected["stdout"])
    assert actual == expected["files"] and directories == expected["directories"], (value.label, "file effects")
    return {"returncode": 0, "stdout": process.stdout.decode(), "stderr": "",
            "files": {name: {"sha256": digest(data), "bytes": len(data)} for name, data in sorted(actual.items())},
            "directories": directories}


def cases() -> list[Case]:
    datasets = [[], [(1, 0, 0)], [(1, 25, 10), (2, 26, 11), (3, -100, 2)],
                [(i + 1, amount, i % 3) for i, amount in enumerate([2**62-1, 2**62, 2**63-1, -(2**63), 2**64])],
                [(1, 10**100, 10**90), (2, 10**100, 0)],
                [(1, 2**63-1, 0), (2, 2**63-1, 0)]]
    generator = random.Random(20261021)
    for _ in range(12):
        datasets.append([(i + 1, generator.randrange(-100, 1000), generator.randrange(20))
                         for i in range(generator.randrange(30))])
    result = [case(f"data-{index}-{mode}-{limit}", rows, mode, limit)
              for index, rows in enumerate(datasets) for mode in ("gross", "net") for limit in (0, 10)]
    result += [case("huge-fee", [(1, 10**100+1, 0)], fee=10**100),
               case("huge-limit", [(1, 2**80, 10**90)], limit=10**100),
               case("output-digit-limit", [(1, 10**4299, 0), (2, 10**4299, 0)], repetitions=10)]
    good = case("valid", [(1, 100, 0), (2, 200, 5)])
    malformed = [b"", b"\n", b"1 2\n", b"-1\n", b"x\n", b"1\n", b"1\n1 2\n",
                 b"1\n1 2 3 4\n", b"1\n0 1 0\n", b"1\n1 1 -1\n", b"2\n1 2 0\n1 3 0\n",
                 b"0\ntrailing\n", b"1\n1 _ 0\n", b"1\n1 + 0\n", b"1\n1 1_000 0\n",
                 "1\n1 ١ 0\n".encode(), b"\xff", b"1\n1 " + b"9"*4301 + b" 0\n"]
    result += [Case(f"malformed-{i}", {"records.txt": data, "report.txt": b"keep"}, good.arguments)
               for i, data in enumerate(malformed)]
    for index, separator in enumerate(("\r\n", "\r", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029")):
        result.append(Case(f"line-{index}", {"records.txt": separator.join(["1", "1\u2003100\u20030", ""]).encode()}, good.arguments))
    for position, invalid in ((0, "unknown"), (3, "-1"), (3, "x"), (4, "-1"), (4, "1_0"), (5, "0"), (5, "-1"), (5, "x")):
        arguments = list(good.arguments); arguments[position] = invalid
        result.append(Case(f"argument-{position}-{invalid}", good.files, tuple(arguments)))
    result += [Case("few-arguments", good.files, good.arguments[:-1]),
               Case("many-arguments", good.files, (*good.arguments, "extra")),
               Case("missing-input", {"report.txt": b"keep"}, good.arguments),
               Case("input-directory", {}, good.arguments, ("records.txt",)),
               Case("output-directory", good.files, good.arguments, ("report.txt",)),
               Case("output-missing-parent", good.files, ("net", "records.txt", "missing/report.txt", "10", "25", "2")),
               Case("output-empty-path", good.files, ("net", "records.txt", "", "10", "25", "2")),
               Case("overwrite-input", good.files, ("net", "records.txt", "records.txt", "10", "25", "2")),
               Case("hardlink-output", good.files, good.arguments, hardlinks={"report.txt": "records.txt"}),
               Case("nested-path", good.files, ("net", "folder/../records.txt", "folder/report.txt", "10", "25", "2"), ("folder",)),
               Case("unicode-path", {"注文.txt": good.files["records.txt"]}, ("net", "注文.txt", "結果.txt", "10", "25", "2"))]
    assert len({value.label for value in result}) == len(result)
    return result


def validate_commands(commands, timeout=30) -> dict:
    results = []
    for value in cases():
        expected = oracle(value)
        engines = {}
        for name, command in commands.items():
            with tempfile.TemporaryDirectory(prefix="record-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                         cwd=directory, timeout=timeout, check=False)
                engines[name] = check_result(value, expected, process, directory)
        results.append({"case": case_record(value), "engines": engines})
    return {"cases": len(results), "engines": list(commands), "results": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    commands = {"cpython": python_command(args.python)}
    if args.binary:
        commands["relar"] = [str(args.binary.resolve(strict=True))]
    if args.rust:
        commands["rust"] = [str(args.rust.resolve(strict=True))]
    result = validate_commands(commands)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Passed {result['cases']} record pipeline cases on {', '.join(commands)}")


if __name__ == "__main__":
    main()
