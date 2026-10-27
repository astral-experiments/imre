"""Measure each pinned test_keyword body at O0 and O3 with its observed fixture."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import (NativeCompileError, NativeDefaultMethod, Toolchain, compile_blocker,
                          method_bytes, run_in_fixtures)

SOURCE = HERE / "upstream/Lib/test/test_keyword.py"
MODULE = "test.test_keyword"
CLASS = "Test_iskeyword"
FIXTURE = (HERE / "fixture.py").read_bytes()
CONTROL = b"""
f = Fixture()
failures = 0
for assertion, args in [(f.assertTrue, (False,)), (f.assertFalse, (True,)),
                        (f.assertIn, ("x", [])), (f.assertListEqual, (["x"], ["y"]))]:
    try:
        assertion(*args)
    except AssertionError:
        failures += 1
try:
    with f.assertRaises(KeyError):
        pass
except AssertionError:
    failures += 1
try:
    with f.assertRaises(KeyError):
        raise ValueError("other exception")
except ValueError:
    failures += 1
with f.assertRaises(KeyError):
    raise KeyError("expected")
trace = []
f.addCleanup(trace.append, 1)
f.addCleanup(trace.append, 2)
f.doCleanups()
assert failures == 6 and trace == [2, 1]
print("fixture verified")
"""


def child(binary):
    return subprocess.run([binary], capture_output=True, text=True, timeout=60,
                          env={"PATH": "/no-host", "LC_ALL": "C.UTF-8"})


def measure(tools, reference, blob, root):
    leaves = list(cases(load_suite(MODULE)))
    discovered = [(case.id(), f"{MODULE}.{CLASS}") for case in leaves]
    observations, jobs = {}, {}
    control = child(tools.build(FIXTURE + CONTROL, root))
    if (control.returncode, control.stdout, control.stderr) != (0, "fixture verified\n", ""):
        raise ValueError(f"native fixture negative controls failed: {control}")
    for case in leaves:
        name, key = case._testMethodName, (case.id(), 1)
        path = root / name
        path.mkdir()
        operation = "keyword predicates and exported lists"
        default = NativeDefaultMethod(tools, path, SOURCE, blob, ("imre", operation)).run(case)
        default["evidence"]["compiler_mode"] = "reachable" if tools.reachable_only else "ordinary"
        if default["outcome"] == "pass":
            observations[key] = default
            continue
        body = method_bytes(SOURCE, CLASS, name)
        entry = FIXTURE + f"\nclass {CLASS}(Fixture):\n".encode() + body + f"""
case = {CLASS}()
hard, soft = keyword.kwlist, keyword.softkwlist
print("entered")
try:
    case.{name}()
finally:
    case.doCleanups()
assert keyword.kwlist is hard and keyword.softkwlist is soft
print("completed")
""".encode()
        try:
            binary = tools.build(entry, path)
        except NativeCompileError as exc:
            observed = compile_blocker(exc)
            observed["reason"] = observed["reason"].replace(str(path), "<case>")
            observed["evidence"]["default_runner"] = default
            observations[key] = observed
            continue

        def execute(case, original, *, binary=binary, body=body, default=default, path=path):
            result = child(binary)
            entered = result.stdout.startswith("entered\n")
            passed = (result.returncode, result.stdout, result.stderr) == (0, "entered\ncompleted\n", "")
            loc = "imre" if entered else "unobserved"
            row = {"outcome": "pass" if passed else "error",
                   "roles": roles(body=loc, assertions=loc if passed else "unobserved",
                                  operation=loc if passed else "unobserved", fixture="mixed"),
                   "evidence": {"fixture": "original CPython lifecycle and native assertion/cleanup fixture",
                                "operation": operation, "method_sha256": hashlib.sha256(body).hexdigest(),
                                "default_runner": default, "opt_level": tools.level,
                                "compiler_mode": "reachable" if tools.reachable_only else "ordinary"}}
            if not passed:
                row["reason"] = f"native exit {result.returncode}: " + result.stderr.replace(str(path), "<case>").strip()
            return row

        jobs[case.id()] = execute
    observed, failures, skips = run_in_fixtures(load_suite(MODULE), jobs)
    if failures or skips:
        raise RuntimeError((failures, skips))
    for name, (row,) in observed.items():
        observations[name, 1] = row
    return build_report(reference, tools.level, discovered, observations)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--cpython-source", type=Path)
    p.add_argument("--reachable-only", action="store_true")
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("test-enabled CPython 3.14.5 required")
    verify_files(HERE / "upstream", manifest["files"])
    if args.cpython_source:
        verify_files(args.cpython_source, manifest["files"])
    verify_interpreter_tests(manifest)
    reports = []
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tools = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only)
            reports.append(measure(tools, reference, manifest["files"]["Lib/test/test_keyword.py"], root))
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
