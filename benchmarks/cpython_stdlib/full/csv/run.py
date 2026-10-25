"""Record native CSV reader evidence for every occurrence in CPython's test_csv suite."""
import argparse
import ast
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from native_csv import ADAPTER, METHODS
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, compile_blocker, method_bytes, run_evidence

MODULE = "test.test_csv"
EXCEL = MODULE + ".TestDialectExcel."
SOURCE = HERE / "upstream/Lib/test/test_csv.py"
# Entire methods: their positive rows and their assertRaises/Regex error paths all use the child.
READER_CASES = {"test_read_nul": 5, "test_read_delimiter": 3, "test_read_escape": 10,
                "test_read_space_delimiter": 2, "test_read_eof": 6, "test_read_eol": 7}


def completed(binary, directory, label):
    proc = subprocess.run([binary, label], cwd=directory, capture_output=True, text=True, timeout=30)
    if proc.returncode or proc.stderr or proc.stdout != f"completed {label}\n":
        raise AssertionError((proc.returncode, proc.stdout, proc.stderr))


def native_excel(tools, work):
    """Compile all 18 original definitions once, then run precisely one in each upstream case."""
    definitions = {name: method_bytes(SOURCE, "TestDialectExcel", name) for name in METHODS}
    source = bytearray(b"import sys\n" + ADAPTER + b"\n")
    for body in definitions.values():
        source.extend(body + b"\n")
    source.extend(b'\ncase = Case()\nname = sys.argv[1]\n')
    for index, name in enumerate(definitions):
        source.extend(f"{'elif' if index else 'if'} name == {name!r}:\n    case.{name}()\n".encode())
    source.extend(b'else:\n    raise ValueError("unknown test")\nassert case.checks == 1\nprint("completed " + name)\n')
    binary = tools.build(source, work)

    def exercise(case, original):
        name = case._testMethodName
        completed(binary, work, name)
        return {"roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                "evidence": {"operation": "csv.reader", "native_calls": 1, "opt_level": tools.level,
                             "method_sha256": hashlib.sha256(definitions[name]).hexdigest(),
                             "fixture": "Native Case writes and reopens its text file; the compiled method supplies the original input and expected rows.",
                             "assertions": "Each row length and field is checked in the native child."}}

    return dict.fromkeys((EXCEL + name for name in METHODS), exercise)


def multiline(tools, work):
    """Preserve the original method's local 'input'; transfer its two fixture arguments."""
    def exercise(case, original):
        calls = []

        def check(data, expected):
            source = ADAPTER + (f"case = Case()\ncase.readerAssertEqual({data!r}, {expected!r})\n"
                                "assert case.checks == 1\nprint('completed multiline')\n").encode()
            binary = tools.build(source, work)
            completed(binary, work, "multiline")
            calls.append(1)

        with patch.object(case, "readerAssertEqual", check):
            original()
        if len(calls) != 1:
            raise AssertionError(f"unexpected native multiline calls: {len(calls)}")
        return {"roles": roles(body="cpython", assertions="imre", operation="imre", fixture="mixed"),
                "evidence": {"operation": "csv.reader", "native_calls": 1, "opt_level": tools.level,
                             "fixture": "The unchanged method supplies multiline input and expected rows on CPython; the native child opens the file.",
                             "assertions": "Each row length and field is checked in the native child."}}

    return exercise


def upstream_reader(tools, work):
    """Use one native reader binary per dialect. Files transport input elements including NUL."""
    binaries = {}

    def native_rows(lines, options, calls):
        option_args = ", ".join(f"{key}={value!r}" for key, value in options.items())
        if option_args not in binaries:
            directory = work / f"dialect-{len(binaries)}"
            directory.mkdir()
            arguments = "lines" + (", " + option_args if option_args else "")
            source = (HERE / "reader.py").read_bytes()
            if source.count(b"csv.reader(lines)") != 1:
                raise ValueError("missing CSV reader call in driver")
            source = source.replace(b"csv.reader(lines)", f"csv.reader({arguments})".encode())
            binaries[option_args] = tools.build(source, directory)
        paths = []
        for index, line in enumerate(lines):
            path = work / f"line-{index}"
            path.write_bytes(line.encode("utf-8"))
            paths.append(path)
        result = subprocess.run([binaries[option_args], *paths], cwd=work,
                                check=True, capture_output=True, text=True, timeout=30)
        if result.stderr:
            raise AssertionError(result.stderr)
        calls.append(1)
        for record in result.stdout.splitlines():
            tag, encoded = record.split(" ", 1)
            value = ast.literal_eval(encoded)
            if tag == "error" and isinstance(value, str):
                raise csv.Error(value)
            if tag != "row" or not isinstance(value, list):
                raise ValueError(f"invalid native row: {record!r}")
            yield value

    def exercise(case, original):
        calls = []
        def invoke(lines, **options):
            return native_rows(lines, options, calls)

        with patch.object(csv, "reader", invoke):
            original()
        if len(calls) != READER_CASES[case._testMethodName]:
            raise AssertionError(f"{case.id()}: observed {len(calls)} native reads")
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "csv.reader", "native_calls": len(calls), "opt_level": tools.level,
                             "fixture": "Original unittest method supplies input strings and dialect options. A file per input string preserves NUL and physical line endings.",
                             "assertions": "Upstream assertEqual and assertRaises/Regex check the returned rows or native error."}}
    return exercise


def reference_skips(level):
    fixture = MODULE + ".TestLeaks"
    return {(fixture + "." + name, 1, level): {
        "kind": "upstream-skip", "fixture_variant": fixture, "platform": "linux",
        "configuration": "CPython 3.14.5 release build without Py_REF_DEBUG",
        "condition": "not hasattr(sys, 'gettotalrefcount')",
        "reason": "requires sys.gettotalrefcount()", "source": "Lib/test/test_csv.py:1503",
    } for name in ("test_create_read", "test_create_write", "test_read", "test_write")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if (reference["module"], reference["cpython_commit"]) != (MODULE, manifest["commit"]):
        raise ValueError("reference does not match pinned CSV release")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reports = []
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory(prefix="imre-csv-evidence-") as tmp:
            root = Path(tmp)
            workbook = root / "excel"
            workbook.mkdir()
            blocked = {}
            try:
                selected = native_excel(tools, workbook)
            except NativeCompileError as exc:
                selected = {}
                blocked = {(EXCEL + name, 1): compile_blocker(exc) for name in METHODS}
            selected[EXCEL + "test_quoted_nl"] = multiline(tools, root)
            selected.update(dict.fromkeys((MODULE + ".Test_Csv." + name for name in READER_CASES),
                                          upstream_reader(tools, root)))
            report = run_evidence(load_suite(MODULE), selected, reference, level, preflight=blocked,
                                  exceptions=reference_skips(level))
            reports.append(report)
            print(f"O{level}: {report['counts']}")
    write_reports(args.result, reports, reference)
    for report in reports:
        for row in report["records"]:
            if row["outcome"] not in {"pass", "not-run"}:
                print(row["id"], row.get("reason", row["reference"]), file=sys.stderr)
        if report["counts"] != {"fully-native": 18, "host-driven-native-operation": 7,
                                "reference-only": 99, "reference-skip": 4}:
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
