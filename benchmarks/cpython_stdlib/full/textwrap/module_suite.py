"""Run the staged, unmodified textwrap module on Imre at O0 and O3."""
import argparse
import importlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent), str(HERE.parents[1])]
from stage_tests import stage
from suite_evidence import build_report, occurrence_keys, reference_cases, roles, write_reports
from suite_inventory import cases, load_suite
from suite_native import NativeCompileError, Toolchain

MODULE = "test.test_textwrap"
MODULE_ENTRY = b"""import unittest
import test.test_textwrap as target

suite = unittest.defaultTestLoader.loadTestsFromModule(target)
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    raise SystemExit(1)
"""


def run_level(tools, directory, source, expected):
    binary = tools.build(source, directory)
    run = subprocess.run([binary, "-v"], capture_output=True, text=True, timeout=90)
    lines = run.stderr.splitlines()
    print(run.stdout, end="")
    print(run.stderr, file=sys.stderr, end="")
    records = [line for line in lines if " ... " in line]
    wanted = [f"{name.rsplit('.', 1)[1]} ({name}) ... ok" for name in expected]
    totals = [int(m[1]) for line in lines if (m := re.fullmatch(r"Ran (\d+) tests? in [\d.]+s", line))]
    if run.returncode or run.stdout or lines[-1:] != ["OK"] or records != wanted or totals != [len(expected)]:
        raise ValueError(f"native exit {run.returncode}, expected {len(wanted)} ordered passing IDs, saw {len(records)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--classes", nargs="*", metavar="CLASS",
                        help="use unittest.main with imported classes (default: all classes when supplied)")
    parser.add_argument("--result", type=Path, help="write paired evidence after both native runs pass")
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != manifest["tag"]:
        parser.error("run with CPython " + manifest["tag"])
    if sys.platform != reference["platform"] or manifest["commit"] != reference["cpython_commit"]:
        parser.error("reference platform or CPython commit mismatch")
    reports = []
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        stage(directory)
        lib = directory / "Lib"
        sys.path.insert(0, str(lib))
        module = importlib.import_module(MODULE)
        if Path(module.__file__).resolve() != lib / "test/test_textwrap.py":
            raise ValueError("CPython did not discover the staged module")
        discovered = [(case.id(), type(case).__module__ + "." + type(case).__qualname__)
                      for case in cases(load_suite(MODULE))]
        all_keys = list(occurrence_keys(name for name, _ in discovered))
        if set(all_keys) != reference_cases(reference).keys():
            raise ValueError("staged discovery differs from pinned reference")
        names = sorted({variant.rsplit(".", 1)[1] for _, variant in discovered})
        source, label, selected = MODULE_ENTRY, "module loader", names
        if args.classes is not None:
            selected = args.classes or names
            if set(selected) - set(names) or len(set(selected)) != len(selected):
                parser.error(f"expected unique class names from {names}")
            label = "imported classes " + ", ".join(selected)
            source = (f"import unittest\nfrom {MODULE} import {', '.join(selected)}\nunittest.main()\n").encode()
        keys = [k for k, (_, variant) in zip(all_keys, discovered, strict=True)
                if variant.rsplit(".", 1)[1] in selected]
        for level in (0, 3):
            print(f"-O{level} {label} ({len(keys)}/{len(all_keys)} occurrences)", flush=True)
            tools = Toolchain(args.imre, args.runtime, args.clang, level, reachable_only=True, module_root=lib)
            try:
                run_level(tools, lib, source, [name for name, _ in keys])
            except (NativeCompileError, ValueError, subprocess.TimeoutExpired) as exc:
                print(exc, file=sys.stderr)
                continue
            observations = {
                key: {
                    "outcome": "pass",
                    "roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                    "evidence": {"operation": "textwrap", "fixture": key[0].rsplit(".", 1)[0] + (", native unittest.main()" if args.classes is not None else ", native module loader")},
                }
                for key in keys
            }
            reports.append(build_report(reference, level, discovered, observations))
    if len(reports) != 2:
        return 1
    if args.result:
        write_reports(args.result, reports, reference)
    return 0


if __name__ == "__main__":
    sys.exit(main())
