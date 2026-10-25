"""Run unchanged launch-error cases and verify the failing syscall in Imre's child."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from attempt_bodies import SOURCE, fixture
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

IDS = {
    f"test.test_subprocess.{cls}.{method}"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
    for method in ("test_file_not_found_includes_filename", "test_file_not_found_with_bad_cwd")
} | {
    f"test.test_subprocess.POSIXProcessTestCase.{method}"
    for method in ("test_exception_bad_args_0", "test_exception_bad_executable", "test_exception_cwd")
}
ACTOR = b"""import subprocess
import sys
try:
    args = sys.argv[3:]
    mode = sys.argv[1]
    if mode == "call":
        subprocess.call(args)
    elif mode == "cwd":
        subprocess.Popen(args, cwd=sys.argv[2])
    elif mode == "executable":
        subprocess.Popen(args, executable=sys.argv[2])
    else:
        subprocess.Popen(args)
except FileNotFoundError as error:
    print("FileNotFoundError")
    code = error.errno
    reason = error.strerror
    name = error.filename
    if code is not None:
        print(code)
    else:
        print("None")
    if reason is not None:
        print(repr(reason))
    else:
        print("None")
    if isinstance(name, str):
        print(repr(name))
    else:
        print("None")
    print(error.filename2 is None)
    print(repr(str(error)))
else:
    raise AssertionError("launch unexpectedly succeeded")
"""


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def measure(binary, directory, case, original, tc):
    from test import test_subprocess as upstream

    # The initial shared attempt includes the complete unchanged helpers. A
    # decorator or a missing compiler feature is recorded, never removed.
    with tempfile.TemporaryDirectory(dir=directory) as temp:
        work = Path(temp)
        try:
            owner, preamble = fixture(case, work)
            execute = native_method(tc, work, SOURCE, owner, case.id(), preamble)
        except (RuntimeError, ValueError) as exc:
            body = {"status": "blocked", "reason": str(exc).replace(str(work), "<case>").strip().splitlines()[-1]}
        else:
            body = {"status": "completed", **execute(case, original)}
    binding = upstream.subprocess._PopenSelector.__name__
    assert binding == ("SelectSelector" if type(case).__name__.endswith("NoPoll") else "PollSelector")
    evidence = []

    def call(operation, args, **options):
        if type(args) is not list or not all(type(s) is str for s in args):
            raise AssertionError(("unexpected argv representation", args))
        if set(options) not in (set(), {"cwd"}, {"executable"}):
            raise AssertionError(("unexpected options", options))
        if operation == "call" and options:
            raise AssertionError(("unexpected call options", options))
        mode, value = next(iter(options.items()), (operation, ""))
        with tempfile.TemporaryDirectory(dir=directory) as temp:
            logs = Path(temp)
            trace = ["execve", "chdir", "clone", "clone3", "fork", "vfork", "wait4",
                     "waitid", "poll", "ppoll", "select", "pselect6"]
            completed = subprocess.run(
                ["strace", "-ff", "-qq", "-s", "1024", "-e", "signal=none", "-e",
                 "trace=" + ",".join(trace), "-o", logs / "pid", "--", binary, mode, value, *args],
                check=True, capture_output=True, text=True, timeout=30,
            )
            if completed.stderr:
                raise AssertionError(completed.stderr)
            kind, code, reason, filename, no_second, display = completed.stdout.splitlines()
            if kind != "FileNotFoundError" or no_second != "True":
                raise AssertionError(completed.stdout)
            # All three public fields must cross the boundary from the native
            # exception. Never fill the filename or errno from the input argv.
            err = FileNotFoundError(int(code), ast.literal_eval(reason), ast.literal_eval(filename))
            if str(err) != ast.literal_eval(display):
                raise AssertionError((err, display))
            traces = {int(f.suffix[1:]): f.read_text().splitlines() for f in logs.glob("pid.*")}
            native_exec = "execve(" + json.dumps(str(binary)) + ","
            [parent] = [pid for pid, lines in traces.items()
                        if any(line.startswith(native_exec) and line.endswith("= 0") for line in lines)]
            syscall = "chdir" if mode == "cwd" else "execve"
            target = options.get("cwd", options.get("executable", args[0]))
            prefix = syscall + "(" + json.dumps(target)
            [child] = [pid for pid, lines in traces.items() if pid != parent and
                       any(line.startswith(prefix) and "= -1 ENOENT " in line for line in lines)]
            if not any(re.match(r"(clone3?|vfork|fork)\(", line) and
                       re.search(rf"= {child}$", line) for line in traces[parent]):
                raise AssertionError("the failing syscall was not in the native parent's child")
            if not any(line.startswith(f"wait4({child},") and re.search(rf"= {child}$", line)
                       for line in traces[parent]):
                raise AssertionError("the failed launch was not reaped by its native parent")
            if any(re.match(r"(poll|ppoll|select|pselect6)\(", line) for line in traces[parent]):
                raise AssertionError("unexpected selector invocation before the failed launch")
        evidence.append(dict(api=operation, syscall=syscall, errno=err.errno, strerror=err.strerror,
                             filename=err.filename, filename2=err.filename2, failed_child_reaped=True,
                             native_parent_selector_calls=0))
        raise err

    bridge = SimpleNamespace(
        Popen=lambda args, **kw: call("Popen", args, **kw),
        call=lambda args, **kw: call("call", args, **kw),
    )
    with patch.object(upstream, "subprocess", bridge):
        original()
    if len(evidence) != 1:
        raise AssertionError(evidence)
    return {"body_attempt": body, "fixture_selector": binding, "observed": evidence[0],
            "roles": dict(body="cpython", assertions="cpython", fixture="cpython", operation="imre")}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + flag, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, IMRE_PYTHON=sys.executable):
        work = Path(temp)
        actor = tc.build(ACTOR, work)
        def execute(case, original):
            return measure(actor, work, case, original, tc)
        rows, failures, skipped = run_in_fixtures(load_suite(manifest["module"]), dict.fromkeys(IDS, execute))
        if rows.keys() != IDS or failures or skipped:
            raise AssertionError((failures, skipped, IDS - rows.keys()))
        result = dict(cpython_commit=manifest["commit"], opt_level=tc.level, actor_sha256=digest(actor),
                      toolchain={k: digest(getattr(args, k)) for k in ("imre", "runtime", "clang")}, rows=rows)
    args.result.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
