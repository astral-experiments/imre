"""Audit the public doctest calls made by the two unchanged CPython load_tests hooks."""

import argparse
import ast
import doctest
import importlib
import json
import json.decoder
import json.encoder
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
DOCSTRINGS = HERE.parents[3] / "crates/imre-compiler/stdlib/docstrings"
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests


def examples(case):
    return [{"source": ex.source, "want": ex.want, "exc_msg": ex.exc_msg,
             "options": ex.options, "lineno": ex.lineno, "indent": ex.indent}
            for ex in case._dt_test.examples]


def audit(test_module):
    module = importlib.import_module(test_module)
    tree = ast.parse(Path(module.__file__).read_bytes())
    api = sorted({node.func.attr for node in ast.walk(tree) if isinstance(node, ast.Call)
                  and isinstance(node.func, ast.Attribute) and isinstance(node.func.value, ast.Name)
                  and node.func.value.id == "doctest"})
    calls = []
    original = doctest.DocTestSuite

    def construct(target, *args, **kwargs):
        if args or kwargs:
            raise AssertionError("load_tests changed its DocTestSuite options")
        suite = original(target)
        calls.append({"module": target.__name__, "ids": [case.id() for case in cases(suite)]})
        return suite

    with patch.object(doctest, "DocTestSuite", construct):
        all_cases = list(cases(load_suite(test_module)))
    ids = []
    docs = {}
    for case in all_cases:
        if not isinstance(case, doctest.DocTestCase):
            continue
        name = case.id()
        if (DOCSTRINGS / (name + ".txt")).read_bytes() != case._dt_test.docstring.encode():
            raise AssertionError(f"bundled native docstring differs from CPython: {name}")
        ids.append(name)
        data = examples(case)
        if docs.setdefault(name, data) != data:
            raise AssertionError(f"repeated doctest {name} has different examples")
        result = unittest.TestResult()
        case.run(result)
        if (result.testsRun, result.errors, result.failures, result.skipped) != (1, [], [], []):
            raise AssertionError(f"{name}: {result.errors}, {result.failures}, {result.skipped}")
    return {"public_calls": api, "suite_instances": len(all_cases), "constructors": calls,
            "doctest_occurrences": ids, "reference_examples": docs}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-source", type=Path, required=True)
    parser.add_argument("--write", action="store_true", help="write the verified reference audit")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the test-enabled CPython 3.14.5 interpreter")
    manifest = json.loads((HERE / "AUDITED_SOURCES.json").read_text())
    verify_files(args.cpython_source, manifest["files"])
    verify_files(DOCSTRINGS, {"LICENSE": manifest["files"]["LICENSE"]})
    if (DOCSTRINGS / "json.decoder.txt").read_bytes() != json.decoder.__doc__.encode():
        raise AssertionError("bundled native json.decoder docstring differs from CPython")
    verify_interpreter_tests(manifest)
    # Verify the interpreter's own modules as well as the supplied source tree.
    for module in (doctest, json, json.encoder, json.decoder):
        path = Path(module.__file__)
        relative = "Lib/" + str(path.relative_to(Path(doctest.__file__).parent))
        verify_files(path.parent, {path.name: manifest["files"][relative]})
    result = {name: audit(name) for name in ("test.test_itertools", "test.test_json")}
    output = json.dumps(result, indent=2, ensure_ascii=True) + "\n"
    baseline = HERE / "reference.json"
    if args.write:
        baseline.write_text(output)
    elif baseline.read_text() != output:
        raise SystemExit("doctest discovery, reference result, or an original example changed")
    for module, entry in result.items():
        occurrences = entry["doctest_occurrences"]
        print(f"{module}: {len(occurrences)}/{entry['suite_instances']} doctest occurrences; "
              f"{sum(len(entry['reference_examples'][n]) for n in occurrences)} examples")


if __name__ == "__main__":
    main()
