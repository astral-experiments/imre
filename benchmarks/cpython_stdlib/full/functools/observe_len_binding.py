"""Observe the live len binding in the two unchanged CPython reference methods."""

import ast
import builtins
import inspect
import json
from pathlib import Path
import sys
import textwrap
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files, verify_interpreter_tests


def observe(case):
    method = getattr(case, case._testMethodName)
    source, first_line = inspect.getsourcelines(method)
    node, = ast.parse(textwrap.dedent("".join(source))).body
    statement, = (stmt for stmt in node.body if isinstance(stmt, ast.Try))
    loop, = (stmt for stmt in statement.body if isinstance(stmt, ast.For))
    assertion, = loop.body
    restore, = statement.finalbody
    # Line events precede the statement, so these reads cannot alter the cache.
    assertion_line, restore_line = (first_line + stmt.lineno - 1 for stmt in (assertion, restore))
    original = builtins.len
    calls, snapshots = [], []

    def trace(frame, event, arg):
        if frame.f_code is not method.__code__:
            return None
        if event == "line" and frame.f_lineno in (assertion_line, restore_line):
            wrapper = builtins.len
            if wrapper is original or wrapper.__wrapped__ is not original:
                raise AssertionError("the live binding did not wrap the captured original len")
            if frame.f_lineno == assertion_line:
                calls.append(frame.f_locals[loop.target.id])
            else:
                snapshots.append(tuple(wrapper.cache_info()))
        return trace

    previous = sys.gettrace()
    try:
        sys.settrace(trace)
        result = unittest.TestResult()
        case.run(result)
    finally:
        sys.settrace(previous)
    if result.testsRun != 1 or not result.wasSuccessful() or result.skipped:
        raise AssertionError((result.errors, result.failures, result.skipped))
    if builtins.len is not original:
        raise AssertionError("finally did not restore the original len by identity")
    snapshot, = snapshots
    return {"id": case.id(), "assertions_while_replaced": calls, "cache_info_before_restore": snapshot,
            "original_is_callback": True, "original_identity_restored": True}


if __name__ == "__main__":
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("the reference requires CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import test_functools

    print(json.dumps({"cpython_commit": manifest["commit"], "operation": "cpython",
                      "occurrences": [observe(cls("test_lru_reentrancy_with_len")) for cls in
                                      (test_functools.TestLRUC, test_functools.TestLRUPy)]}, indent=2))
