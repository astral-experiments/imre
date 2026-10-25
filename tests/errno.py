"""Compare complete unmodified errno methods, failing controls, and the platform's real registry."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parents[1] / "benchmarks/cpython_stdlib/full/errno"


def run(*args, cwd=None):
    return subprocess.run(args, cwd=cwd, capture_output=True, check=False)


def observe(result):
    return result.returncode, result.stdout, result.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--result-dir", type=Path)
    args = parser.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source = HERE / "upstream/Lib/test/test_errno.py"
    data = source.read_bytes()
    digest = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    assert digest == manifest["files"]["Lib/test/test_errno.py"]
    tests = sorted(reference["reference"])
    assert len(tests) == reference["denominator"]
    with tempfile.TemporaryDirectory(prefix="imre-errno-") as tmp:
        root = Path(tmp)
        # Only its module name changes; the source, fixture, classes and method bodies do not.
        shutil.copyfile(source, root / "suite.py")
        shutil.copyfile(HERE / "driver.py", root / "driver.py")
        shutil.copyfile(HERE / "operations.py", root / "operations.py")
        cases = [("driver", test.rsplit(".", 1)[1]) for test in tests]
        cases += [("driver", "missing-code"), ("driver", "missing-attribute"), ("operations",)]
        oracle = {c: observe(run(args.python, "-E", root / (c[0] + ".py"), *c[1:], cwd=root)) for c in cases}
        for c in cases[:len(tests)]:
            assert oracle[c] == (0, b"pass\n", b""), (c, oracle[c])
        for c in cases[len(tests):-1]:
            status, stdout, stderr = oracle[c]
            assert status == 0 and not stderr and stdout.startswith(b"assertion: "), (c, oracle[c])
        assert oracle[("operations",)][0] == 0 and not oracle[("operations",)][2]
        print(f"CPython: {len(tests)} original methods, {len(cases) - len(tests) - 1} negative controls, {len(oracle[('operations',)][1].splitlines())} registry observations")
        for opt in args.opt_levels:
            for prog in sorted({c[0] for c in cases}):
                command = [args.compiler.resolve(), "build", root / (prog + ".py"), "--module-root", root,
                           "--reachable-only", "--runtime", args.runtime.resolve(), "--clang", args.clang,
                           "--opt-level", opt, "--no-cache", "-o", root / prog]
                if args.sanitize:
                    command.append("--sanitize")
                result = run(*command, cwd=root)
                if result.returncode:
                    sys.stderr.buffer.write(result.stderr)
                    return 1
            for c in cases:
                actual = observe(run(root / c[0], *c[1:], cwd=root))
                if actual != oracle[c]:
                    raise AssertionError((opt, c, oracle[c], actual))
            print(f"O{opt}: {len(tests)}/{len(tests)} original methods, 2 failure controls, complete platform registry matched")
            if args.result_dir:
                args.result_dir.mkdir(exist_ok=True, parents=True)
                report = {"cpython_commit": reference["cpython_commit"], "source_blob": digest,
                          "denominator": len(tests), "opt_level": opt, "platform": sys.platform,
                          "execution": "fresh native process per original method, direct method driver",
                          "tests": {t: {"status": "native"} for t in tests},
                          "controls": {"missing-code": "matched", "missing-attribute": "matched"},
                          "registry_observations": len(oracle[("operations",)][1].splitlines())}
                (args.result_dir / f"native-{sys.platform}-O{opt}.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
