"""Full reports, typed JSON failures, and filesystem effects for memory_report."""

import argparse
import collections
import json
import os
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
PACKAGE = PACKAGE_ROOT / "memory_report"
ENTRY = PACKAGE / "__main__.py"
STDLIB = ROOT / "stdlib"
UPSTREAM = PACKAGE / "upstream/memory_report.py"
files = SimpleNamespace(**runpy.run_path(str(HERE / "manifest_validate.py")))
Case = files.Case
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
encoded_files = files.encoded_files
digest = files.digest
BOOTSTRAP = (
    "import runpy, sys; sys.path[:0] = sys.argv[1:3]; "
    "sys.argv = ['memory_report', *sys.argv[3:]]; "
    "runpy.run_module('memory_report', run_name='__main__', alter_sys=True)"
)


def python_command(python):
    return [python, "-I", "-B", "-c", BOOTSTRAP, str(PACKAGE_ROOT), str(STDLIB)]


def original_command(python):
    # -I would ignore PYTHONHASHSEED and randomize upstream's equal-change ties.
    return [python, "-B", str(UPSTREAM), "compare"]


def environment():
    result = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONPATH": ""}
    result.pop("PYTHONHOME", None)
    return result


def report(total=0, structs=None, queries=None):
    return {"total_bytes": total, "struct_metadata_bytes": 0, "struct_fields_bytes": 0,
            "memo_metadata_bytes": 0, "memo_fields_bytes": 0,
            "structs": structs or [], "queries": queries or []}


def case(label, old, new, output="report.md"):
    encode = lambda value: value if isinstance(value, bytes) else (json.dumps(value, ensure_ascii=True) + "\n").encode()
    arguments = ("--old", "old.json", "--new", "project.json")
    initial = {"old.json": encode(old), "project.json": encode(new)}
    if output is not None:
        arguments += ("--output", output)
        initial[output] = b"replace the complete report"
    return Case(label, initial, arguments)


def integer(text):
    negative = text.startswith("-")
    digits = text[1:] if negative else text
    value = 0
    for start in range(0, len(digits), 9):
        chunk = digits[start:start + 9]
        value = value * 10 ** len(chunk) + int(chunk)
    return -value if negative else value


def format_bytes(value):
    value = float(value)
    for unit in ("B", "kB", "MB"):
        if abs(value) < 1024:
            return f"{value:.2f}{unit}"
        value /= 1024
    return f"{value:.2f}GB"


def format_diff(old, new):
    difference = new - old
    if not difference:
        return "-"
    sign = "+" if difference > 0 else ""
    if not old:
        return f"{sign}{format_bytes(difference)} (new)"
    return f"{sign}{difference / old:.2%} ({format_bytes(abs(difference))})"


def outcome(old, new):
    return "⏫" if new > old else "⬇️" if new < old else "✅"


def expected_report(old, new, name):
    """Use plain dictionaries and tuple rows, independently of the typed port."""
    old_total, new_total = old["total_bytes"], new["total_bytes"]
    if abs(new_total - old_total) <= 10_000:
        return "## Memory usage report\n\nMemory usage unchanged ✅", []
    lines = ["## Memory usage report", "", "### Summary", "",
             "| Project | Old | New | Diff | Outcome |",
             "|---------|-----|-----|------|---------|",
             f"| {name} | {format_bytes(old_total)} | {format_bytes(new_total)} | {format_diff(old_total, new_total)} | {outcome(old_total, new_total)} |",
             "", "### Significant changes", "", "<details>",
             "<summary>Click to expand detailed breakdown</summary>", ""]
    old_items = {item["name"]: item.get("metadata_bytes", 0) + item.get("fields_bytes", 0)
                 for item in old.get("structs", []) + old.get("queries", [])}
    new_items = {item["name"]: item.get("metadata_bytes", 0) + item.get("fields_bytes", 0)
                 for item in new.get("structs", []) + new.get("queries", [])}
    names = dict.fromkeys([*old_items, *new_items])
    changes = [(name, old_items.get(name, 0), new_items.get(name, 0)) for name in names
               if old_items.get(name, 0) != new_items.get(name, 0)]
    changes.sort(key=lambda item: abs(item[2] - item[1]), reverse=True)
    cutoff = abs(changes[min(14, len(changes) - 1)][2] - changes[min(14, len(changes) - 1)][1]) if changes else 0
    rows = [(f"| `{name.replace(' ', '')}` | {format_bytes(before)} | {format_bytes(after)} | {format_diff(before, after)} |{outcome(before, after)} |", abs(after - before))
            for name, before, after in changes if abs(after - before) >= cutoff]
    if rows:
        lines.extend([f"### {name}", "", "| Name | Old | New | Diff | Outcome |",
                      "|------|-----|-----|------|---------|"])
        lines.extend(row for row, _ in rows[:15])
        if len(changes) > 15:
            lines.append(f"| ... | | | *{len(changes) - 15} more* |")
        lines.append("")
    lines.extend(["</details>", ""])
    return "\n".join(lines), rows


def options(value):
    arguments = value.arguments[1:] if value.arguments[:1] == ("compare",) else value.arguments
    return dict(zip(arguments[::2], arguments[1::2], strict=True))


def project_name(path):
    # Spell out Python 3.14 Path.stem so the oracle also runs on older hosts.
    name = path.rsplit("/", 1)[-1]
    dot = name.rfind(".")
    return name[:dot] if dot >= 0 and any(char != "." for char in name[:dot]) else name


def oracle(value, error=None):
    expected = {"error": error, "files": dict(value.files),
                "directories": sorted(files.directory_names(value) - {"."})}
    for name, target in value.hardlinks.items():
        expected["files"][name] = expected["files"][target]
    if error:
        return expected
    flags = options(value)
    old = json.loads(value.files[flags["--old"]], parse_int=integer)
    new = json.loads(value.files[flags["--new"]], parse_int=integer)
    rendered, rows = expected_report(old, new, project_name(flags["--new"]))
    expected.update(rendered=rendered, rows=rows, output=flags.get("--output"))
    return expected


def apply_output(value, expected, rendered):
    result = dict(expected["files"])
    output = expected["output"]
    if output is None:
        return result
    output = os.path.normpath(output)
    data = rendered.encode()
    target = value.hardlinks.get(output, output)
    result[target] = data
    for name, linked in value.hardlinks.items():
        if linked == target:
            result[name] = data
    return result


def equivalent_original(rendered, expected):
    """Allow only upstream's set-dependent order/selection among equal changes."""
    actual_lines = rendered.splitlines(keepends=True)
    canonical_lines = expected["rendered"].splitlines(keepends=True)
    is_row = lambda line: line.startswith("| `")
    assert [line for line in actual_lines if not is_row(line)] == [line for line in canonical_lines if not is_row(line)]
    actual_rows = [line.rstrip("\n") for line in actual_lines if is_row(line)]
    available = collections.Counter(row for row, _ in expected["rows"])
    assert not (collections.Counter(actual_rows) - available)
    magnitudes = {row: magnitude for row, magnitude in expected["rows"]}
    assert [magnitudes[row] for row in actual_rows] == [magnitude for _, magnitude in expected["rows"][:15]]


def check_result(value, expected, process, directory, engine="port"):
    actual, directories = files.snapshot(directory)
    stdout, stderr = process.stdout.decode(), process.stderr.decode()
    if expected["error"]:
        assert process.returncode != 0 and not stdout, (value.label, engine, process)
        category = expected["error"]
        allowed = {"OSError": r"(?:OSError|FileNotFoundError|IsADirectoryError|NotADirectoryError)",
                   "ValueError": r"(?:ValueError|UnicodeDecodeError)"}.get(category, category)
        assert re.search(allowed + r":", stderr), (value.label, engine, stderr)
        expected_files = expected["files"]
    else:
        assert process.returncode == 0, (value.label, engine, process)
        if engine == "original":
            if expected["output"] is None:
                assert not stderr and stdout.endswith("\n")
                rendered = stdout[:-1]
            else:
                prefix = f"Output written to {expected['output']}\n"
                assert not stdout and stderr.startswith(prefix) and stderr.endswith("\n")
                rendered = stderr[len(prefix):-1]
            equivalent_original(rendered, expected)
        else:
            rendered = expected["rendered"]
            assert not stderr and stdout == (rendered + "\n" if expected["output"] is None else ""), (value.label, engine, stdout, stderr)
        expected_files = apply_output(value, expected, rendered)
    assert actual == expected_files and directories == expected["directories"], (value.label, engine, "file effects")
    return {"returncode": process.returncode, "stdout": stdout, "stderr": stderr,
            "files": {name: {"sha256": digest(data), "bytes": len(data)} for name, data in sorted(actual.items())},
            "directories": directories}


def cases():
    result = []
    def add(value, error=None, original=True):
        result.append((value, error, original and error is None))
    for name in ("inventory", "ruff-scripts"):
        old = (PACKAGE / f"sample/{name}-old.json").read_bytes()
        new = (PACKAGE / f"sample/{name}.json").read_bytes()
        add(case("real-" + name, old, new))
        add(case("real-stdout-" + name, old, new, None))
    for difference in (-10001, -10000, 0, 10000, 10001):
        add(case(f"threshold-{difference}", report(20000), report(20000 + difference)))
    for count in (0, 1, 15, 16, 40):
        old = report(100000, [{"name": f"item {i}", "metadata_bytes": i * 100 + 1} for i in range(count)])
        new = report(200000, [{"name": f"item {i}", "metadata_bytes": i * 100 + (i % 3) * 17 + 1} for i in range(count)])
        add(case(f"ties-and-cutoff-{count}", old, new))
    add(case("added-removed-duplicate-unicode", report(0, [{"name": "α β", "fields_bytes": 1}, {"name": "gone", "metadata_bytes": 7}, {"name": "α β", "metadata_bytes": 8}]),
             report(20000, [{"name": "α β", "fields_bytes": 10}, {"name": "new🙂", "metadata_bytes": 99}])) )
    for index, number in enumerate((2**53 + 1, 2**63 - 1, 2**63, 2**64, 10**100, -(2**63), -(10**100))):
        add(case(f"wide-{index}", report(number, [{"name": "wide", "metadata_bytes": number, "fields_bytes": number}]),
                 report(number + 20001, [{"name": "wide", "metadata_bytes": number, "fields_bytes": number + 12345}])))
    randomizer = random.Random(20261021)
    for index in range(20):
        count = randomizer.randrange(40)
        old = report(randomizer.randrange(-100000, 100001), [{"name": f"item {i % 23}", "metadata_bytes": randomizer.randrange(-100000, 100001)} for i in range(count)])
        new = report(randomizer.randrange(-100000, 100001), [{"name": f"item {i % 29}", "fields_bytes": randomizer.randrange(-100000, 100001)} for i in range(count)])
        add(case(f"random-{index}", old, new))
    minimal = {key: 0 for key in report() if key not in ("structs", "queries")}
    add(case("missing-list-defaults", minimal, {**minimal, "total_bytes": 20000, "structs": [{"name": "default"}]}))
    good = json.dumps(report(20000)).encode()
    for index, malformed in enumerate((b"", b"null", b"[]", b"{}", b"true", b"1", b"{", b"\xff",
                                       good + b" trailing", good.replace(b'20000', b'true', 1), good.replace(b'20000', b'2e4', 1),
                                       good.replace(b'[]', b'null', 1), good.replace(b'[]', b'[{}]', 1),
                                       good.replace(b'20000', b'{"$serde_json::private::Number":"20000"}', 1))):
        add(case(f"invalid-{index}", report(), malformed), "ValueError")
    duplicate = good[:-1] + b',"total_bytes":20000}'
    add(case("duplicate-known", report(), duplicate), "ValueError")
    add(case("duplicate-unknown", report(), good[:-1] + b',"unknown":1,"unknown":2}'))
    add(case("ignored-surrogate", report(), good[:-1] + b',"unknown":{"x":"\\ud800"}}'))
    for field in (b'"name":"\\ud800"', b'"name":"ok","metadata_bytes":null'):
        add(case("invalid-item-" + str(len(result)), report(), good.replace(b'"structs": []', b'"structs": [{' + field + b'}]')), "ValueError")
    for depth in (127, 128):
        text = good[:-1] + b',"unknown":' + b'[' * depth + b'0' + b']' * depth + b'}'
        add(case(f"depth-{depth + 1}", report(), text), "ValueError" if depth == 128 else None)
    huge = b'1' + b'0' * 5000
    suppressed = good.replace(b'20000', huge, 1)
    add(case("unbounded-integer-suppressed", suppressed, suppressed), original=False)
    add(case("float-overflow-before-output", report(), suppressed), "OverflowError")
    normal = case("normal", report(0), report(20000, [{"name": "x", "fields_bytes": 3}]))
    add(Case("overwrite-old", normal.files, ("--old", "old.json", "--new", "project.json", "--output", "old.json")))
    add(Case("hardlink-output", {key: value for key, value in normal.files.items() if key != "report.md"}, normal.arguments,
             hardlinks={"report.md": "project.json"}))
    add(Case("missing-old", {"report.md": b"keep"}, normal.arguments), "OSError")
    add(Case("output-directory", {key: value for key, value in normal.files.items() if key != "report.md"}, normal.arguments, ("report.md",)), "OSError")
    add(Case("output-missing-parent", normal.files, ("--old", "old.json", "--new", "project.json", "--output", "missing/report.md")), "OSError")
    add(Case("unknown-flag", normal.files, (*normal.arguments, "--bad", "x")), "ValueError")
    assert len({value.label for value, _, _ in result}) == len(result)
    return result


def validate_commands(commands, timeout=30):
    results = []
    for value, error, original in cases():
        expected = oracle(value, error)
        engines = {}
        for name, command in commands.items():
            if name == "original" and not original:
                continue
            with tempfile.TemporaryDirectory(prefix="memory-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                         cwd=directory, env=environment(), timeout=timeout, check=False)
                engines[name] = check_result(value, expected, process, directory, name)
        results.append({"case": case_record(value), "expected_error": error, "engines": engines})
    return {"cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "results": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    commands = {"original": original_command(args.python), "port": python_command(args.python)}
    if args.binary:
        commands["relar"] = [str(args.binary.resolve(strict=True))]
    if args.rust:
        commands["rust"] = [str(args.rust.resolve(strict=True))]
    result = validate_commands(commands)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Passed {result['cases']} memory report cases, {result['engine_executions']} engine executions")


if __name__ == "__main__":
    main()
