"""Replay the two pinned CPython SHAKE vector tests on one native Imre executable."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from stage_tests import stage

# python/pythontestdotnet at a226968296ab3054903aa4d0b453714310ebf3e7, www/hashlib/.
VECTORS = {
    "shake_128": ("test_case_shake128_vector", "79b573c11d0e7ced041833a41998bbaab569f36ade48354a088e634521813e97"),
    "shake_256": ("test_case_shake256_vector", "19682db629598b99db52d3636fa738394559401bb64d8fb25d60c2df36a6fc43"),
}
SUITE = """
assert sys.modules["test.test_hashlib"] is target
first = target.HashLibTestCase("test_case_shake128_vector")
second = target.HashLibTestCase("test_case_shake256_vector")
assert first.id() == "test.test_hashlib.HashLibTestCase.test_case_shake128_vector"
assert second.id() == "test.test_hashlib.HashLibTestCase.test_case_shake256_vector"
suite = unittest.TestSuite([first, second])
assert suite.countTestCases() == 2
result = unittest.TextTestRunner(verbosity=2).run(suite)
assert result.testsRun == 2
if not result.wasSuccessful():
    raise SystemExit(1)
"""


def run(executable, lib, failed=None, digests=()):
    proc = subprocess.run([executable], cwd=lib, text=True, capture_output=True, timeout=600)
    expected = [
        (method, f"test.test_hashlib.HashLibTestCase.{method}", "FAIL" if method == failed else "ok")
        for method, _ in VECTORS.values()
    ]
    rows = re.findall(r"^(\w+) \(([\w.]+)\) \.\.\. (.+)$", proc.stderr, re.MULTILINE)
    summary = "FAILED (failures=1)" if failed else "OK"
    if (rows != expected or proc.returncode != int(failed is not None)
            or not proc.stderr.rstrip().endswith("\n" + summary)
            or not all(digest in proc.stderr for digest in digests)):
        raise RuntimeError(f"Unexpected SHAKE result (exit {proc.returncode})\n{proc.stdout}{proc.stderr}")
    for _, case_id, status in rows:
        print(f"{case_id} ... {status}")
    print(f"exit {proc.returncode}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "vectors"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    parser.add_argument("--runtime-imports", action="store_true")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        stage(root)
        lib = root / "Lib"
        cache = lib / "test/data"
        cache.mkdir(exist_ok=True)
        for name, (_, checksum) in VECTORS.items():
            filename = name + ".txt"
            data = (args.vectors / filename).read_bytes()
            if hashlib.sha256(data).hexdigest() != checksum:
                raise ValueError(f"Wrong upstream vector file: {filename}")
            (cache / filename).write_bytes(data)

        entry = lib / "imre_shake_vectors.py"
        if args.runtime_imports:
            imports = (
                'from importlib import import_module\n'
                'target = import_module("test.test_hashlib")\n'
                'sys = import_module("sys")\n'
                'unittest = import_module("unittest")\n'
            )
        else:
            imports = 'import test.test_hashlib as target\nimport sys\nimport unittest\n'
        entry.write_text(imports + SUITE)
        exe = root / "shake_vectors"
        cmd = [args.imre.resolve(), "build", entry, "--module-root", lib, "--runtime",
               args.runtime.resolve(), "--clang", args.clang.resolve(), "--no-cache",
               "--opt-level", args.opt_level, "-o", exe]
        if args.runtime_imports:
            cmd.append("--runtime-imports")
        subprocess.run(cmd, cwd=lib, check=True, timeout=600)

        run(exe, lib)
        for name, (method, _) in VECTORS.items():
            path = cache / (name + ".txt")
            original = path.read_bytes()
            # Each pinned file ends with the answer for the 255-byte message.
            prefix, sep, tail = original.rpartition(b",")
            correct = tail.splitlines()[0].decode("ascii")
            wrong = f"{int(correct[0], 16) ^ 1:x}" + correct[1:]
            path.write_bytes(prefix + sep + tail.replace(correct.encode(), wrong.encode()))
            run(exe, lib, failed=method, digests=(correct, wrong))
            path.write_bytes(original)


if __name__ == "__main__":
    main()
