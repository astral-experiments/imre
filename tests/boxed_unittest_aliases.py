"""Compare normal boxed unittest imports, duplicate registration, skips and fixture effects."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

from unittest_main import report


FIXTURES = Path(__file__).resolve().parent / "fixtures/unittest_boxed_aliases"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    entry = FIXTURES / "driver.py"
    reference = subprocess.run([args.python, "-EB", entry], capture_output=True, text=True, timeout=30)
    assert reference.returncode == 1 and "SUMMARY 12 2 0 8 False" in reference.stdout, reference
    assert reference.stdout.count("FAILED real_cases.Active.test_fail\n") == 2, reference
    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / "suite"
        llvm = Path(tmp) / "suite.ll"
        for level in (0, 3):
            for flags in ([], ["--reachable-only"]):
                subprocess.run([
                    args.compiler, "build", entry, "--module-root", FIXTURES,
                    "--no-cache", "--opt-level", str(level), "--runtime", args.runtime,
                    "--clang", args.clang, *flags, "--emit-llvm", llvm, "-o", binary,
                ], check=True, timeout=240)
                with llvm.open("rb") as emitted:
                    if not any(b"call ptr @imre_dyn_new(" in line for line in emitted):
                        raise AssertionError("expected ordinary imports to exercise the boxed runtime")
                result = subprocess.run([binary], capture_output=True, text=True, timeout=30)
                if (result.returncode != reference.returncode or result.stdout != reference.stdout
                        or report(result.stderr, True) != report(reference.stderr, True)):
                    raise AssertionError((level, flags, reference, result))
                print(f"-O{level} {flags}: 10 alias occurrences matched", flush=True)


if __name__ == "__main__":
    main()
