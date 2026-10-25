"""Compare the mutable export list and deprecated ExecError lookup with CPython."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from copyfile import STDLIB, build
from suite_inventory import verify_files, verify_interpreter_tests
from suite_native import NativeCompileError

SOURCES = {
    "exports": b'''import shutil as fs
from shutil import __all__ as names

def check() -> None:
    print("|".join(names))
    print(names is fs.__all__)
    names.append("fixture added this name")
    print(fs.__all__[-1])
    print(fs.__all__.pop())
    print("|".join(names))

check()
''',
    "deprecated_lookup": b'''import shutil
import warnings
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always", DeprecationWarning)
    from shutil import ExecError as legacy
    print(legacy is RuntimeError)
    print(shutil.ExecError is legacy)
    for item in caught:
        print(item.category.__name__)
        print(str(item.message))
with warnings.catch_warnings():
    warnings.simplefilter("error", DeprecationWarning)
    try:
        from shutil import ExecError as repeated
    except DeprecationWarning as error:
        print(str(error))
''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=lambda p: Path(p).resolve())
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run with pinned CPython 3.14.5")
    manifest = json.loads((Path(__file__).resolve().parent / "UPSTREAM.json").read_bytes())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    report = {"cpython_commit": manifest["commit"], "controls": {}}
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        for name, source in SOURCES.items():
            script = root / (name + ".py")
            script.write_bytes(source)
            ref = subprocess.run([sys.executable, "-IB", script], capture_output=True, check=True)
            assert not ref.stderr
            lines = ref.stdout.decode().splitlines()
            if name == "exports":
                assert lines[0] == lines[-1] and lines[1:4] == ["True", "fixture added this name", "fixture added this name"]
            else:
                assert lines[:2] == ["True", "True"] and lines.count("DeprecationWarning") == 2
                assert lines[3] == lines[5] == lines[6]
            levels = {}
            for level in (0, 3):
                try:
                    binary = build(args, root, f"{name}-{level}", source, level)
                except NativeCompileError as error:
                    levels[str(level)] = {"status": "compile-blocked", "diagnostic": str(error)}
                    continue
                child = subprocess.run([binary], capture_output=True, timeout=30)
                matched = child.returncode == 0 and child.stdout == ref.stdout and child.stderr == ref.stderr
                levels[str(level)] = {"status": "pass" if matched else "failed", "exit": child.returncode,
                                      "stdout": child.stdout.decode(), "stderr": child.stderr.decode()}
            report["controls"][name] = {"reference_stdout": lines,
                "source_sha256": hashlib.sha256(source).hexdigest(), "levels": levels}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(r["status"] == "failed" for c in report["controls"].values() for r in c["levels"].values()))


if __name__ == "__main__":
    sys.exit(main())
