"""Run unchanged CPython copy methods with typed native assertions."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, compile_blocker, method_bytes, run_evidence

# Each body keeps its original inputs and expected values. For exact int lists
# and str -> int dicts the specialized assertions compare lengths and all entries.
CASES = {
    "test_copy_basic": ("int", "assert left == right", 1),
    "test_copy_list": ("list[int]", "assert len(left) == len(right)\n"
                       "        for i in range(len(left)):\n            assert left[i] == right[i]", 4),
    "test_copy_dict": ("dict[str, int]", "assert len(left) == len(right)\n"
                       "        for key in left:\n            assert key in right and left[key] == right[key]", 4),
    "test_copy_set": ("set[int]", "assert left == right", 4),
}


def native_case(tools, work, name, ty, equal, checks):
    method = method_bytes(HERE / "upstream/Lib/test/test_copy.py", "TestCopy", name)
    header = f"""import copy

class Case:
    checks: int
    def __init__(self) -> None:
        self.checks = 0
    def assertEqual(self, left: {ty}, right: {ty}) -> None:
        {equal}
        self.checks += 1
"""
    if ty != "int":
        header += f"""    def assertIsNot(self, left: {ty}, right: {ty}) -> None:
        assert left is not right
        self.checks += 1
"""
    entry = f"\ncase = Case()\ncase.{name}()\nassert case.checks == {checks}\nprint('completed')\n"
    binary = tools.build(header.encode() + method + entry.encode(), work)
    def exercise(case, original):
        run = subprocess.run([binary], cwd=work, capture_output=True, text=True, timeout=30)
        if run.returncode or run.stderr or run.stdout != "completed\n":
            raise AssertionError((run.returncode, run.stdout, run.stderr))
        return {"roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                "evidence": {"operation": "copy.copy", "opt_level": tools.level,
                             "method_sha256": hashlib.sha256(method).hexdigest(),
                             "assertions": f"{checks} native TestCase assertions completed.",
                             "fixture": "The original method creates its own objects in the native process."}}
    return exercise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    module = manifest["module"]
    reports = []
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            selected, blocked = {}, {}
            for name, (ty, equal, checks) in CASES.items():
                work = Path(tmp) / name
                work.mkdir()
                key = module + ".TestCopy." + name
                try:
                    selected[key] = native_case(tools, work, name, ty, equal, checks)
                except NativeCompileError as exc:
                    blocked[key, 1] = compile_blocker(exc)
            report = run_evidence(load_suite(module), selected, reference, level, preflight=blocked)
            reports.append(report)
            print(f"O{level}: {report['counts']}", flush=True)
            for row in report["records"]:
                if row["outcome"] not in {"pass", "not-run"}:
                    print(row["id"], row.get("reason"), file=sys.stderr)
    write_reports(args.result, reports, reference)
    return int(any(r["counts"].get("fully-native") != len(CASES) for r in reports))


if __name__ == "__main__":
    sys.exit(main())
