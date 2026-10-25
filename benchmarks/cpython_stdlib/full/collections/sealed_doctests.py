"""Compare genuine DocTestSuite discovery, example metadata and execution on CPython and Imre."""
import argparse
import collections
import doctest
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

DISCOVER = """print(suite.countTestCases())
for case in suite:
    print(case.id())
"""
METADATA = """for case in suite:
    print(case.id())
    found = case._dt_test
    print(found.docstring)
    for example in found.examples:
        print(example.source)
        print(example.want)
        message = example.exc_msg
        if message is None:
            print("<no exception>")
        else:
            print(message)
        print(example.options)
        print(example.lineno)
        print(example.indent)
"""

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--compiler-revision", required=True)
    p.add_argument("--require-discovery", action="store_true")
    args = p.parse_args()
    assert sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 5)
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    assert (HERE / "upstream/Lib/collections/__init__.py").read_bytes() == Path(collections.__file__).read_bytes()
    refs = json.loads((HERE / "reference-linux.json").read_bytes())["reference"]
    cases = list(doctest.DocTestSuite(collections))
    assert {c.id() for c in cases} == {name for name in refs if name.startswith("collections.")}
    catalogs = HERE.parents[3] / "crates/imre-compiler/stdlib/docstrings"
    for case in cases:
        assert (catalogs / (case.id() + ".txt")).read_bytes() == case._dt_test.docstring.encode()
    def header(mod):
        return f"import doctest\nimport {mod}\nsuite = doctest.DocTestSuite({mod})\n"
    scripts = {f"discover-{mod}": header(mod) + DISCOVER
               for mod in ("collections", "itertools", "json", "json.encoder", "json.decoder")}
    scripts.update(metadata=header("collections") + METADATA,
                   elements=header("collections") + "for case in suite:\n    if case.id() == 'collections.Counter.elements':\n        case.debug()\n        print(case.id())\n",
                   execute=header("collections") + "suite.debug()\n")
    imre, runtime, clang = (f.resolve() for f in (args.imre, args.runtime, args.clang))
    report = {"compiler_revision": args.compiler_revision, "cpython_commit": manifest["commit"],
              "imre_sha256": digest(imre), "runtime_sha256": digest(runtime),
              "examples": {case.id(): len(case._dt_test.examples) for case in cases}, "entries": {}}
    for name, script in scripts.items():
        with tempfile.TemporaryDirectory(prefix="collections-doctest-") as parent:
            work = Path(parent)
            control = subprocess.run([sys.executable, "-E", "-B", "-c", script], cwd=work,
                                     text=True, capture_output=True, check=True)
            entry = {"source": script, "reference_stdout_sha256": hashlib.sha256(control.stdout.encode()).hexdigest(),
                     "reference_stdout_bytes": len(control.stdout.encode()), "native": {}}
            report["entries"][name] = entry
            for opt in (0, 3):
                for reach in (False, True):
                    mode = f"O{opt}-" + ("reachable" if reach else "whole")
                    d = work / mode
                    d.mkdir()
                    try:
                        binary = Toolchain(imre, runtime, clang, opt, reach).build(script.encode(), d)
                    except NativeCompileError as exc:
                        row = {"status": "compile-blocked", "reason": str(exc).replace(str(d), "<entry>").strip()}
                    else:
                        got = subprocess.run([binary], cwd=d, text=True, capture_output=True, timeout=90)
                        matched = (got.returncode, got.stdout, got.stderr) == (0, control.stdout, control.stderr)
                        row = {"status": "matched" if matched else "failed", "exit": got.returncode,
                               "stdout_sha256": hashlib.sha256(got.stdout.encode()).hexdigest(), "stderr": got.stderr}
                        if not matched:
                            row["stdout"] = got.stdout
                    entry["native"][mode] = row
                    print(name, mode, row["status"], flush=True)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    assert all(row["status"] != "failed" for entry in report["entries"].values()
               for row in entry["native"].values()), "native output differed from CPython"
    if args.require_discovery:
        assert all(row["status"] == "matched" for name, e in report["entries"].items()
                   if name.startswith("discover-") or name == "metadata" for row in e["native"].values())
    assert report["imre_sha256"] == digest(imre) and report["runtime_sha256"] == digest(runtime)

if __name__ == "__main__":
    main()
