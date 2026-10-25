"""Trace the native parent to prove when a NoPoll fixture cannot affect the tested operation."""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

NAMES = (
    "test_call_kwargs",
    "test_call_seq",
    "test_check_call_zero",
    "test_communicate_returns",
    "test_communicate_stderr",
    "test_communicate_stdin",
    "test_communicate_stdout",
    "test_stderr_pipe",
    "test_stdin_pipe",
    "test_stdout_pipe",
    "test_stdout_stderr_pipe",
    "test_wait",
)
CONTROL = "test.test_subprocess.POSIXProcessTestCase.test_communicate_repeated_call_after_stdout_close"


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--mode", choices=("build", "trace"), required=True)
    for n in ("imre", "runtime", "clang", "work", "result"):
        p.add_argument("--" + n, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    a = p.parse_args()
    a.work.mkdir(exist_ok=True, parents=True)
    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    toolchain = {name: digest(getattr(a, name)) for name in ("imre", "runtime", "clang")}

    def verify(case, original):
        name = case.id()
        root = a.work / name
        root.mkdir(exist_ok=True)
        owner, preamble = fixture(case, root)
        blob = hashlib.sha256(
            method_bytes(SOURCE, owner, case._testMethodName)
        ).hexdigest()
        stamp = root / "compiled.json"
        provenance = dict(method_sha256=blob, opt_level=a.opt_level, toolchain=toolchain)
        if a.mode == "build":
            native_method(tc, root, SOURCE, owner, name, preamble)
            provenance["executable"] = digest(root / "case")
            stamp.write_text(json.dumps(provenance))
            return {"method_sha256": blob}
        provenance["executable"] = digest(root / "case")
        if json.loads(stamp.read_text()) != provenance:
            raise AssertionError(f"compiled binary or toolchain changed: {name}")
        trace = root / "selector.trace"
        # Do not follow forks: the code under test is the native parent.
        result = subprocess.run(
            [
                "/usr/bin/strace",
                "-qq",
                "-e",
                "signal=none",
                "-e",
                "trace=poll,ppoll,select,pselect6",
                "-o",
                trace,
                "--",
                root / "case",
            ],
            capture_output=True,
            text=True,
            timeout=60,
        )
        if result.returncode or result.stderr or result.stdout != f"completed {name}\n":
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        counts = Counter(
            re.findall(r"^(poll|ppoll|select|pselect6)\(", trace.read_text(), re.M)
        )
        if name == CONTROL and not counts:
            raise AssertionError("missing positive control")
        if name != CONTROL and counts:
            raise AssertionError(
                (
                    "selector reached in native NoPoll case",
                    counts,
                    trace.read_text()[:300],
                )
            )
        return {
            "method_sha256": blob,
            "native_parent_syscalls": dict(counts),
            "fixture": "original CPython module, class and instance",
        }

    ids = [f"test.test_subprocess.ProcessTestCaseNoPoll.{n}" for n in NAMES] + [CONTROL]
    with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
        rows, failures, skips = run_in_fixtures(
            load_suite("test.test_subprocess"), dict.fromkeys(ids, verify)
        )
    if failures or skips or set(rows) != set(ids):
        raise AssertionError((failures, skips))
    a.result.write_text(
        json.dumps({"opt_level": a.opt_level, "mode": a.mode, "ids": rows}, indent=2)
        + "\n"
    )


if __name__ == "__main__":
    main()
