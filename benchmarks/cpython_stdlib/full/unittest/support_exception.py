"""Check the original test.support errors on the argument shapes used by regrtest.

The isolated fixtures are controls, not additional upstream suite occurrences.
The installed CPython classes and their source-exact projections must agree.
"""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
from suite_native import NativeCompileError, Toolchain

COMMIT = "5607950ef232dad16d75c0cf53101d9649d89115"
SUPPORT = "Lib/test/support/__init__.py"
STATS = "Lib/test/libregrtest/result.py"
CALLER = "Lib/test/libregrtest/single.py"
BLOBS = {SUPPORT: "6635ec3474e12ec6a2f684a1a93b6ec08df49a37",
         STATS: "daf7624366ee207382881103e8186b4e2ec6ea57",
         CALLER: "958a915626ad2414ff5b070daf3123f39a0b5330"}

DRIVER = b'''
errors = [("case.one", "trace one")]
failures = [("case.two", "trace two")]
problem = TestFailedWithDetails("failed", errors, failures, stats=stats)
try:
    raise problem
except TestDidNotRun:
    raise AssertionError("sibling matched the child")
except TestFailedWithDetails as child:
    print(child is problem, child.errors is errors, child.failures is failures, child.stats is stats)
    print(child.args[1] is errors, child.args[2] is failures, len(child.args))
    errors.append(("during", "changed"))
    print(str(child), repr(child.args))
try:
    raise problem
except TestFailed as parent:
    print(parent is problem, parent.msg, parent.stats is stats)
parent_error = TestFailed("parent", 17, "tail", stats=stats)
print(str(parent_error), repr(parent_error.args), parent_error.stats is stats)
print(repr(TestFailed("plain").args), TestFailed("plain").stats is None)
'''


def original_classes(path, names):
    raw = path.read_bytes()
    lines = raw.splitlines(keepends=True)
    nodes = {node.name: node for node in ast.parse(raw).body if isinstance(node, ast.ClassDef)}
    first, last = nodes[names[0]], nodes[names[-1]]
    begin = min([first.lineno, *(d.lineno for d in first.decorator_list)]) - 1
    selected = [n.name for n in nodes.values() if begin < n.lineno <= last.end_lineno]
    if selected != names:
        raise ValueError(f"unexpected declarations in fixture: {selected}")
    return b"".join(lines[begin:last.end_lineno])


def run(args):
    import test

    verify_files(args.cpython_source, BLOBS)
    verify_files(Path(test.__file__).resolve().parent,
                 {name.removeprefix("Lib/test/"): sha for name, sha in BLOBS.items()})
    controls = []
    hierarchy = original_classes(args.cpython_source / SUPPORT,
                                 ["Error", "TestFailed", "TestFailedWithDetails", "TestDidNotRun"])
    stats_class = original_classes(args.cpython_source / STATS, ["TestStats"])
    for variant in ("no-stats", "regrtest-stats"):
        supplied = b"stats = None\n" if variant == "no-stats" else b"stats = TestStats(4, 1, 0)\n"
        imports = b"from test.support import TestFailed, TestFailedWithDetails, TestDidNotRun\n"
        fixture = hierarchy + b"\n"
        if variant == "regrtest-stats":
            imports += b"from test.libregrtest.result import TestStats\n"
            fixture = b"import dataclasses\n" + stats_class + b"\n" + fixture
        source = fixture + supplied + DRIVER
        def oracle(code):
            return subprocess.run([sys.executable, "-E", "-B", "-c", code],
                                  capture_output=True, check=True, timeout=30)
        installed = oracle(imports + supplied + DRIVER)
        projected = oracle(source)
        if (installed.stdout, installed.stderr) != (projected.stdout, projected.stderr):
            raise ValueError("projected original classes differ from the installed reference")
        observation = {"variant": variant, "source_sha256": hashlib.sha256(source).hexdigest(),
                       "hierarchy_sha256": hashlib.sha256(hierarchy).hexdigest(),
                       "reference_stdout": projected.stdout.decode(), "native": []}
        if args.emit:
            args.emit.mkdir(parents=True, exist_ok=True)
            (args.emit / ("projection_" + variant.replace("-", "_") + ".py")).write_bytes(source)
        for reachable in (False, True) if args.imre else ():
            for level in (0, 3) if args.runtime else (None,):
                with tempfile.TemporaryDirectory(prefix="imre-support-exception-") as work:
                    root = Path(work)
                    entry = root / "case.py"
                    entry.write_bytes(source)
                    mode = {"reachable_only": reachable, "opt_level": level}
                    if level is None:
                        r = subprocess.run([args.imre, "check", entry, *(["--reachable-only"] if reachable else [])],
                                           capture_output=True, timeout=120)
                        mode.update(stage="check", status=r.returncode, diagnostic=r.stderr.decode().replace(work, "<fixture>"))
                    else:
                        try:
                            binary = Toolchain(args.imre, args.runtime, args.clang, level, reachable).build(source, root)
                        except NativeCompileError as exc:
                            mode.update(stage="compile", outcome="blocked", reason=str(exc).replace(work, "<fixture>"))
                        else:
                            r = subprocess.run([binary], capture_output=True, timeout=30)
                            matched = (r.returncode, r.stdout, r.stderr) == (0, projected.stdout, projected.stderr)
                            mode.update(stage="run", outcome="matched" if matched else "mismatch", status=r.returncode,
                                        stdout=r.stdout.decode(errors="backslashreplace"), stderr=r.stderr.decode(errors="backslashreplace"))
                    observation["native"].append(mode)
        controls.append(observation)
    return {"cpython_commit": COMMIT, "original_blobs": BLOBS, "upstream_occurrences": 0,
            "operation": "test.support exception construction, forwarding, identity and catches",
            "reference": "installed stock error classes compared with their byte-identical source projections",
            "controls": controls}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("cpython-source", "result", "imre", "runtime", "clang", "emit"):
        parser.add_argument("--" + name, type=Path, required=name in ("cpython-source", "result"))
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or bool(args.runtime) != bool(args.clang) or (args.runtime and not args.imre):
        parser.error("use CPython 3.14.5; native execution requires --imre, --runtime and --clang together")
    report = run(args)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    if args.runtime and any(m.get("outcome") != "matched" for c in report["controls"] for m in c["native"]):
        sys.exit(1)
