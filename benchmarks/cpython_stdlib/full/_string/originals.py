"""Run the original tests; CPython hosts unittest, the child supplies both _string operations."""
import argparse
import ast
import collections
import hashlib
import inspect
import json
from pathlib import Path
import string
import subprocess
import unittest
from unittest.mock import patch
from test import test_str
from test.test_string import test_string

HERE = Path(__file__).resolve().parent


def blob(path):
    data = Path(path).read_bytes()
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def pinned(module, suite, relative):
    root = HERE.parent / suite
    expected = json.loads((root / "UPSTREAM.json").read_text())["files"][relative]
    assert blob(module.__file__) == expected == blob(root / "upstream" / relative)


class Rows:
    """Replay each native result at its actual pull, including recovery after errors."""
    def __init__(self, lines, corrupt=False):
        assert lines[:1] == ["True"]  # iter(cursor) is cursor
        assert lines[-2:] == ["end", "True"]  # next(exhausted, None)
        self.events = iter(lines[1:-2])
        self.corrupt = corrupt

    def __iter__(self):
        return self

    def __next__(self):
        event = next(self.events)
        if event.startswith("error:"):
            raise ValueError(event.removeprefix("error:"))
        row = ast.literal_eval(event)
        if self.corrupt:
            self.corrupt = False
            return ("intentionally wrong", None, None, None)
        return row


class Native:
    def __init__(self, exe, corrupt=None):
        self.exe = str(exe)
        self.corrupt = corrupt
        self.calls = collections.Counter()

    def invoke(self, name, text):
        self.calls[name] += 1
        if type(text) is int and text == 1:
            errors = subprocess.check_output([self.exe, "--integer-one"], text=True).splitlines()
            assert len(errors) == 2
            raise TypeError(errors[0 if name == "parse" else 1])
        assert type(text) is str, ("unrepresented test input", type(text))
        lines = subprocess.check_output([self.exe, "--one", text], text=True).splitlines()
        assert lines[0] == "parse"
        boundary = lines.index("split")
        if name == "parse":
            return Rows(lines[1:boundary], self.corrupt == name)
        first, *rest = lines[boundary + 1:]
        if first.startswith("initial error:"):
            raise ValueError(first.removeprefix("initial error:"))
        return ("intentionally wrong" if self.corrupt == name else ast.literal_eval(first), Rows(rest))

    def formatter_parser(self, text, /):
        return self.invoke("parse", text)

    def formatter_field_name_split(self, text, /):
        return self.invoke("split", text)


def run(case, bridge):
    result = unittest.TestResult()
    with patch.object(test_str, "_string", bridge), patch.object(string, "_string", bridge):
        case.run(result)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("executable", type=Path)
    parser.add_argument("--emit-parse", type=Path)
    args = parser.parse_args()
    pinned(test_str, "str", "Lib/test/test_str.py")
    pinned(test_string, "string", "Lib/test/test_string/test_string.py")
    assert blob(string.__file__) == "eab5067c9b133ea1210efacd92eb5f69c2336236"
    if args.emit_parse:
        method = "".join(inspect.getsourcelines(string.Formatter.parse)[0])
        args.emit_parse.write_text("import _string\nclass Formatter:\n" + method +
            '\nf = Formatter()\nfor row in f.parse("prefix {2!s}xxx{0:^+10.3f}{obj.attr!s} {z[0]!s:10}"):\n    print(row)\n')
    cases = [test_str.StringModuleTest("test_formatter_parser"), test_str.StringModuleTest("test_formatter_field_name_split")]
    names = sorted(n for n in vars(test_string.ModuleTest) if n.startswith("test_") and n not in {"test_lazy_import", "test_attrs", "test_capwords"})
    assert len(names) == 14
    cases += [test_string.ModuleTest(n) for n in names]
    report = {}
    for case in cases:
        native = Native(args.executable)
        result = run(case, native)
        assert result.wasSuccessful() and not result.skipped, (case.id(), result.errors, result.failures, result.skipped)
        report[case.id()] = dict(native.calls)
    for case, operation in zip(cases[:2], ["parse", "split"], strict=True):
        result = run(type(case)(case._testMethodName), Native(args.executable, corrupt=operation))
        assert len(result.failures) == 1 and not result.errors, (case.id(), result.failures, result.errors)
    print(json.dumps(report, sort_keys=True, indent=2))


if __name__ == "__main__":
    main()
