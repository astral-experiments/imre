"""Check whether the native compiler can enter complete, unchanged CPython test suites."""
import argparse
import hashlib
import json
import keyword
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from stage_tests import stage


def suite_name(value):
    if not value.startswith("test.") or not all(
        part.isidentifier() and not keyword.iskeyword(part) for part in value.split(".")
    ):
        raise argparse.ArgumentTypeError("expected a dotted module under test")
    return value


def support_file(value):
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or ".." in path.parts or path.suffix != ".py":
        raise argparse.ArgumentTypeError("expected a Python source path relative to Lib/")
    return str(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--cpython-git", type=Path, help="stage the full Lib/test from this Git repository instead of the vendored fixtures")
    parser.add_argument("--support-file", action="append", type=support_file, default=[],
                        help="also stage this pinned Lib/ source (repeatable)")
    parser.add_argument("suites", nargs="*", type=suite_name, help="defaults to primary wave-one suites")
    args = parser.parse_args()
    if args.support_file and not args.cpython_git:
        parser.error("--support-file needs a --cpython-git source")
    compiler = args.compiler.resolve(strict=True)
    with compiler.open("rb") as binary:
        compiler_digest = hashlib.file_digest(binary, "sha256").hexdigest()
    pin = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())["commit"]
    names = args.suites or ["test." + v for v in tomllib.loads((HERE / "scope.toml").read_text())["wave_one"].values()]
    checks = []
    with tempfile.TemporaryDirectory(prefix="imre-cpython-suite-") as tmp:
        work = Path(tmp)
        if args.cpython_git:
            with tempfile.TemporaryFile() as archive:
                subprocess.run(["git", "--literal-pathspecs", "-C", args.cpython_git, "archive", "--format=tar", pin, "--",
                                "Lib/test", *["Lib/" + name for name in args.support_file]],
                               stdout=archive, check=True)
                archive.seek(0)
                with tarfile.open(fileobj=archive) as tar:
                    tar.extractall(work, filter="data")
        else:
            stage(work)
        root = work / "Lib"
        entry = root / "imre_suite.py"
        for suite in names:
            module = root.joinpath(*suite.split("."))
            if not (module.with_suffix(".py").is_file() or (module / "__init__.py").is_file()):
                parser.error(f"{suite} is not staged; --cpython-git includes the full pinned Lib/test")
            source = f"import {suite} as target\n"
            driver = ("import unittest\nsuite = unittest.defaultTestLoader.loadTestsFromModule(target)\n"
                      "unittest.TextTestRunner(verbosity=2).run(suite)\n")
            for mode, body in [("import", source), ("runner", source + driver)]:
                entry.write_text(body)
                for reachable in (False, True):
                    flags = ["--reachable-only"] if reachable else []
                    proc = subprocess.run([compiler, "check", entry, "--module-root", root, *flags],
                                          capture_output=True, text=True)
                    checks.append({"suite": suite, "entry": mode, "reachable_only": reachable,
                                   "exit": proc.returncode,
                                   "diagnostic": (proc.stdout + proc.stderr).replace(str(root) + "/", "Lib/").strip()})
    print(json.dumps({"cpython_commit": pin, "compiler_sha256": compiler_digest,
                      "operation": "check", "tests_executed": 0, "source": "git" if args.cpython_git else "vendored",
                      "support_files": args.support_file, "checks": checks}, indent=2))
    return int(any(row["exit"] != 0 for row in checks))


if __name__ == "__main__":
    raise SystemExit(main())
