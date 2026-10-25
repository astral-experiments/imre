"""Trace the original outer parent, inner -c subprocess operation and data child."""

import argparse
import ast
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from host_popen import Harness, verify
from selector_fixture import selected_entry, verify_binding
from suite_inventory import load_suite
from suite_native import (
    NativeCompileError,
    Toolchain,
    method_bytes,
    native_method,
    run_in_fixtures,
    testcase_fixture,
)

NAMES = (
    "test_stdout_none",
    "test_stderr_redirect_with_no_stdout_redirect",
    "test_stdout_filedes_of_stdout",
)
SOURCE = HERE / "upstream/Lib/test/test_subprocess.py"
SELECTORS = ("poll", "ppoll", "select", "pselect6")


def programs(method):
    # Adjacent string literals are evaluated by Python's parser. No line of
    # the method, the inner -c program, or its data child's source is rewritten.
    tree = ast.parse("class Original:\n" + method.decode())
    [middle] = [
        n.value
        for n in ast.walk(tree)
        if isinstance(n, ast.Constant)
        and isinstance(n.value, str)
        and "subprocess" in n.value
    ]
    [leaf] = [
        n.value
        for n in ast.walk(ast.parse(middle))
        if isinstance(n, ast.Constant)
        and isinstance(n.value, str)
        and ("import " in n.value or "print(" in n.value)
    ]
    return middle, leaf


def executable(path, text):
    path.write_text("#!/bin/sh\n" + text)
    path.chmod(0o755)
    return path


def source_hash(text):
    return hashlib.sha256(text.encode()).hexdigest()


def build_router(tc, work, name, method):
    routes, details, binaries = [], {}, {}
    for stage, code in zip(("middle", "data_child"), programs(method), strict=True):
        directory = work / stage
        directory.mkdir()
        details[stage] = {"source_sha256": source_hash(code)}
        try:
            binary = tc.build(code.encode(), directory)
            argv = [str(binary)]
            details[stage]["engine"] = "imre"
        except NativeCompileError as exc:
            # This grandchild only writes bytes to stdout. The redirected
            # subprocess.call is in its parent and must always compile.
            if (
                stage != "data_child"
                or name != "test_stdout_filedes_of_stdout"
                or "subprocess" in code
            ):
                raise
            binary = Path(sys.executable)
            argv = [str(binary), "-c", code]
            details[stage].update(
                engine="cpython",
                compile_diagnostic=str(exc)
                .splitlines()[-1]
                .replace(str(directory), "<case>"),
            )
        binaries[stage] = binary
        details[stage]["binary_sha256"] = hashlib.sha256(
            binary.read_bytes()
        ).hexdigest()
        routes.append(
            f'if [ "$2" = {shlex.quote(code)} ]; then exec {shlex.join(argv)}; fi'
        )
    # This fixture is an exact-source dispatcher for these two -c arguments;
    # it does not implement the Python CLI or execute Python expressions.
    router = executable(
        work / "fixture-python",
        '[ "$#" = 2 ] && [ "$1" = -c ] || exit 97\n'
        + "\n".join(routes)
        + "\nexit 98\n",
    )
    return router, binaries, details


class Trace:
    def __init__(self, work):
        self.work = work
        self.parent = None

    def wrap(self, binary):
        if self.parent is not None and self.parent != binary:
            raise AssertionError(
                "original method unexpectedly launched multiple native parents"
            )
        self.parent = binary
        command = [
            "strace",
            "-ff",
            "-qq",
            "-e",
            "signal=none",
            "-e",
            "trace=execve,clone,clone3,fork,vfork,wait4," + ",".join(SELECTORS),
            "-o",
            str(self.work / "syscalls"),
            "--",
            str(binary),
        ]
        return executable(
            self.work / "trace-parent", "exec " + shlex.join(command) + ' "$@"\n'
        )

    def pid_and_selectors(self, binary):
        target = "execve(" + json.dumps(str(binary)) + ","
        matches = []
        for path in self.work.glob("syscalls.*"):
            lines = path.read_text().splitlines()
            if any(line.startswith(target) and line.endswith("= 0") for line in lines):
                syscalls = Counter(
                    line.split("(", 1)[0]
                    for line in lines
                    if line.startswith(tuple(s + "(" for s in SELECTORS))
                )
                matches.append((int(path.suffix[1:]), dict(syscalls)))
        [result] = matches
        return result

    def child_reaped(self, parent, child):
        lines = (self.work / f"syscalls.{parent}").read_text().splitlines()
        created = [
            i
            for i, line in enumerate(lines)
            if re.match(r"(clone3?|fork|vfork)\(", line) and line.endswith(f"= {child}")
        ]
        reaped = [
            i
            for i, line in enumerate(lines)
            if line.startswith(f"wait4({child},") and line.endswith(f"= {child}")
        ]
        assert len(created) == len(reaped) == 1 and created[0] < reaped[0], (
            parent,
            child,
            lines,
        )


def exercise(args, tc, work, cls, method_name):
    from test import test_subprocess as upstream

    key = f"test.test_subprocess.{cls}.{method_name}"
    method = method_bytes(SOURCE, "ProcessTestCase", method_name)
    router, binaries, stages = build_router(tc, work, method_name, method)
    trace = Trace(work)
    no_poll = cls == "ProcessTestCaseNoPoll"

    class TracedToolchain:
        def build(self, source, directory):
            if no_poll:
                source = selected_entry(source)
            return trace.wrap(tc.build(source, directory))

    class TracedHarness(Harness):
        def actor(self, keywords, layout):
            return trace.wrap(super().actor(keywords, layout, select_backend=no_poll))

    harness = TracedHarness(args, work)

    def transfer_fixture():
        verify_binding(upstream.subprocess, no_poll)

    diagnostic = None
    try:
        native_execute = native_method(
            TracedToolchain(),
            work,
            SOURCE,
            "ProcessTestCase",
            key,
            "import subprocess\nimport sys\n" + testcase_fixture(),
            operation=("imre", "outer Popen and inner subprocess operation"),
        )
        body = "imre"

        def execute(case, original):
            transfer_fixture()
            return native_execute(case, original)
    except NativeCompileError as exc:
        diagnostic = str(exc).splitlines()[-1].replace(str(work), "<case>")
        body = "cpython"

        def execute(case, original):
            transfer_fixture()
            with (
                patch.object(upstream, "subprocess", harness),
                patch.object(upstream, "sys", SimpleNamespace(executable=str(router))),
            ):
                original()
            return {"calls": harness.calls}

    try:
        with patch.dict(os.environ, IMRE_PYTHON=str(router)):
            executions, errors, skips = run_in_fixtures(
                load_suite("test.test_subprocess"), {key: execute}
            )
        if errors or skips or executions.keys() != {key}:
            raise AssertionError((key, errors, skips))
    finally:
        for parent in harness.parents:
            parent.finish()
    if body == "cpython" and not {"Popen", "communicate", "returncode"} <= set(
        harness.calls
    ):
        raise AssertionError(harness.calls)
    binaries["outer"] = trace.parent
    stages["outer"] = {
        "engine": "imre",
        "binary_sha256": hashlib.sha256(trace.parent.read_bytes()).hexdigest(),
    }
    pids = {}
    for stage, binary in binaries.items():
        pid, counts = trace.pid_and_selectors(binary)
        if pid in pids.values():
            raise AssertionError("one PID cannot prove two generations of subprocesses")
        pids[stage] = pid
        stages[stage]["observed_execve"] = True
        stages[stage]["selector_syscalls"] = counts
    for parent, child in (("outer", "middle"), ("middle", "data_child")):
        trace.child_reaped(pids[parent], pids[child])
        stages[parent]["created_and_reaped"] = child
    observed = stages["outer"]["selector_syscalls"]
    if no_poll:
        assert not ({"poll", "ppoll"} & observed.keys()), (key, observed)
        assert {"select", "pselect6"} & observed.keys(), (key, observed)
    else:
        assert {"poll", "ppoll"} & observed.keys(), (key, observed)
    return {
        "method_sha256": hashlib.sha256(method).hexdigest(),
        "body_diagnostic": diagnostic,
        "original_output_assertions_passed": True,
        "processes": stages,
        "host_requested_native_calls": harness.calls,
        "roles": {
            "body": body,
            "assertions": body,
            "fixture": "cpython",
            "operation": "imre",
        },
        "outcome": "pass",
        "fixture_selector": ("SelectSelector" if no_poll else "PollSelector"),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = p.parse_args()
    manifest = verify()
    tc = Toolchain(args.imre, args.runtime, args.clang, int(args.opt_level))
    rows = {}
    with tempfile.TemporaryDirectory() as tmp:
        for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll"):
            for method in NAMES:
                key = f"test.test_subprocess.{cls}.{method}"
                with tempfile.TemporaryDirectory(dir=tmp) as work:
                    with patch.dict(
                        os.environ, IMRE_CACHE_DIR=str(Path(work, "cache"))
                    ):
                        rows[key] = exercise(args, tc, Path(work), cls, method)
    args.result.write_text(
        json.dumps(
            {
                "opt_level": int(args.opt_level),
                "cpython_commit": manifest["commit"],
                "rows": rows,
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
