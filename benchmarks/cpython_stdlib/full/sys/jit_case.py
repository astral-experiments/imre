"""Run the original JIT child assertions against the native execution engine."""

import ast
import hashlib
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from test.support import script_helper

from suite_evidence import roles
from suite_native import NativeCompileError, run_in_fixtures

ACTIVE_ID = "test.test_sys.TestSysJIT.test_jit_is_active"
AVAILABLE_ID = "test.test_sys.TestSysJIT.test_jit_is_available"


def observer(module, toolchain, work, pinned_methods):
    # This probes whether the *fixture interpreter* needs its environment.
    # It does not call any JIT predicate under test.
    script_helper.interpreter_requires_environment()
    entry = work / "jit-available"
    entry.mkdir()
    jit = toolchain.build(b"from sys import _jit\nprint(_jit.is_available())\n", entry)

    def jit_case(case, original):
        calls = []
        children = []

        def available():
            process = subprocess.run([jit], capture_output=True, check=True, timeout=15)
            value = ast.literal_eval(process.stdout.decode())
            if process.stderr or type(value) is not bool:
                raise AssertionError((process.stdout, process.stderr))
            calls.append("sys._jit.is_available")
            return value

        def popen(argv, **kwargs):
            if argv[:-1] != [sys.executable, "-X", "faulthandler", "-c"]:
                raise ValueError(f"unexpected fixture invocation: {argv!r}")
            data = argv[-1].encode()
            dest = work / f"{case._testMethodName}-child-{len(children)}"
            dest.mkdir()
            child_record = {"sha256": hashlib.sha256(data).hexdigest(),
                            "PYTHON_JIT": kwargs["env"]["PYTHON_JIT"],
                            "fixture_options": argv[1:-2],
                            "launch": "compile the original -c source directly"}
            children.append(child_record)
            try:
                child = toolchain.build(data, dest)
            except NativeCompileError as exc:
                child_record["diagnostic"] = str(exc).replace(str(dest), "[child]").strip()
                raise
            # The original assert_python_ok controls its environment, pipes,
            # cleanup and status assertion. The child executes the original source.
            return subprocess.Popen([child], **kwargs)

        fixture = SimpleNamespace(Popen=popen, PIPE=subprocess.PIPE, _cleanup=subprocess._cleanup)
        with patch.object(module, "sys", SimpleNamespace(_jit=SimpleNamespace(is_available=available))), \
                patch.object(script_helper, "subprocess", fixture):
            try:
                original()
            except NativeCompileError:
                if (case.id() != ACTIVE_ID or calls != ["sys._jit.is_available"] or len(children) != 1
                        or children[0]["PYTHON_JIT"] != "0" or "_testcapi" not in children[0]["diagnostic"]):
                    raise
                return {"roles": roles(body="cpython", assertions="unobserved", operation="unobserved", fixture="cpython"),
                        "evidence": {"method_sha256": pinned_methods[type(case).__name__, case._testMethodName],
                                     "native_parent_calls": calls, "child_attempts": children,
                                     "native_is_active_calls": 0, "unreached_environment": {"PYTHON_JIT": "1"}}}
        if len(calls) != 1 or [child["PYTHON_JIT"] for child in children] != ["0", "1"]:
            raise AssertionError((calls, children))
        return {"execution": "mixed", "body": "CPython; both child sources: Imre",
                "assertions": "Imre in each child; CPython checks child status",
                "fixture_execution": "CPython unittest hooks and original assert_python_ok",
                "calls": calls, "children": children,
                "observation": {
                    "roles": roles(body="cpython", assertions="mixed", operation="imre", fixture="cpython"),
                    "evidence": {
                        "operation": "native sys._jit.is_available() in the parent query and the native predicate in both children",
                        "fixture": "original CPython unittest lifecycle, assert_python_ok, environment and pipes",
                        "assertions": "both original child assert statements execute in Imre; CPython asserts each exit status",
                        "method_sha256": pinned_methods[type(case).__name__, case._testMethodName],
                        "native_calls": len(calls) + len(children), "children": children, "opt_level": toolchain.level,
                    },
                }}

    return jit_case


def audit_active(suite, selected, preflight):
    """Retain the complete-method diagnostic and observe the original private-API child separately."""
    observed, failures, skips = run_in_fixtures(suite, {ACTIVE_ID: selected[AVAILABLE_ID]})
    if failures or skips or set(observed) != {ACTIVE_ID}:
        raise AssertionError((failures, skips, observed))
    partial, = observed[ACTIVE_ID]
    record = preflight[ACTIVE_ID, 1]
    if record["outcome"] != "blocked" or "exception_proposal" not in record["evidence"]:
        raise AssertionError(record)
    record["roles"] = partial["roles"]
    record["evidence"]["original_fixture"] = partial["evidence"]
