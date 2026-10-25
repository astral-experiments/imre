"""Compare complete unmodified errno methods, failing controls, and the platform's real registry."""
import argparse
import hashlib
import errno
import json
from itertools import zip_longest
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parents[1] / "benchmarks/cpython_stdlib/full/errno"
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, validate_pair


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
    parser.add_argument("--evidence", type=Path, help="write a paired v1 ledger for the reference platform")
    args = parser.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source = HERE / "upstream/Lib/test/test_errno.py"
    data = source.read_bytes()
    digest = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    assert digest == manifest["files"]["Lib/test/test_errno.py"]
    tests = sorted(reference["reference"])
    assert len(tests) == reference["denominator"]
    reports = []
    if args.evidence and (sys.platform != reference["platform"] or args.opt_levels != ["0", "3"]):
        parser.error("paired evidence requires the reference platform and --opt-levels 0 3")
    with tempfile.TemporaryDirectory(prefix="imre-errno-") as tmp:
        root = Path(tmp)
        # Only its module name changes; the source, fixture, classes and method bodies do not.
        shutil.copyfile(source, root / "suite.py")
        shutil.copyfile(HERE / "driver.py", root / "driver.py")
        shutil.copyfile(HERE / "operations.py", root / "operations.py")
        original = [("driver", test.rsplit(".", 1)[1]) for test in tests]
        failures = [("driver", "missing-code"), ("driver", "missing-attribute")]
        cases = [*original, *failures, ("driver", "caught-import"), ("operations",)]
        oracle = {c: observe(run(args.python, "-E", root / (c[0] + ".py"), *c[1:], cwd=root)) for c in cases}
        for c in original:
            assert oracle[c] == (0, b"pass\n", b""), (c, oracle[c])
        for c in failures:
            status, stdout, stderr = oracle[c]
            assert status == 0 and not stderr and stdout.startswith(b"assertion: "), (c, oracle[c])
        assert oracle[("operations",)][0] == 0 and not oracle[("operations",)][2]
        print(f"CPython: {len(tests)} original methods, {len(failures)} negative controls, {len(oracle[('operations',)][1].splitlines())} registry observations")
        # An unsupported symbol is a runtime property of the target. Verify that an unused
        # failing import precedes the next module initializer, including on an entry with no globals.
        symbols = (HERE.parents[3] / "crates/imre-runtime/native/errno-symbols.txt").read_text().splitlines()
        absent = next(line.split()[0] for line in symbols if line.strip() and not line.startswith("#")
                      and not hasattr(errno, line.split()[0]))
        unknown = "a_deliberately_missing_platform_attribute_well_over_32_bytes"
        (root / "side_effect.py").write_text('print("child initialized")\n')
        (root / "prior.py").write_text('print("prior initialized")\n')
        missing = {"platform-missing": absent, "unknown": unknown, "close-spelling": "EDAOM"}
        for case, name in missing.items():
            source = f"from errno import {name} as unused\nimport side_effect\n"
            if case == "close-spelling":
                # Class tuples own entry globals; the import must still run between the two children.
                source = "import prior\n" + source + 'class Local:\n    names = ("one", "two")\nprint(Local.names[0])\n'
            (root / f"{case}.py").write_text(source + "print('entry initialized')\n")
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
            observations = {}
            for c in cases:
                actual = observe(run(root / c[0], *c[1:], cwd=root))
                if actual != oracle[c]:
                    expect = oracle[c]
                    for line, (left, right) in enumerate(zip_longest(expect[1].splitlines(), actual[1].splitlines()), 1):
                        if left != right:
                            left, right = left or b"", right or b""
                            offset = next(i for i, (a, b) in enumerate(zip_longest(left, right)) if a != b)
                            window = slice(max(0, offset - 50), offset + 150)
                            raise AssertionError(f"O{opt} {c}: stdout line {line}, byte {offset}: "
                                                 f"CPython={left[window]!r}, native={right[window]!r}")
                    raise AssertionError((opt, c, "status/stderr", expect[0], actual[0], expect[2][:500], actual[2][:500]))
                if c[0] == "driver" and c[1].startswith("test_"):
                    test = next(t for t in tests if t.rsplit(".", 1)[1] == c[1])
                    observations[test, 1] = {"outcome": "pass", "roles": roles(
                        body="imre", assertions="imre", operation="imre", fixture="imre"),
                        "evidence": {"operation": "native errno object backed by target headers; "
                                                 "original method completed in its own executable process",
                                     "fixture": "Imre imported the intact module as suite, created its "
                                               "original module-level frozenset and unittest case, and ran "
                                               "native unittest assertions; a host driver scheduled each ID",
                                     "source_blob": digest, "process_stdout": actual[1].decode().strip()}}
            for mode in ("optional-hint", "optional-module", "optional-spelling"):
                expected = run(args.python, "-E", root / "driver.py", mode, cwd=root)
                actual = run(root / "driver", mode, cwd=root)
                assert expected.returncode == actual.returncode == 1 and expected.stdout == actual.stdout == b""
                message = expected.stderr.splitlines()[-1:]
                assert message == [b"ImportError: cannot import" + (b". Did you mean: 'EDOM'?" if mode == "optional-hint" else b"")]
                assert actual.stderr.splitlines() == message, (expected.stderr, actual.stderr)
            for case in missing:
                command = [args.compiler.resolve(), "build", root / (case + ".py"), "--module-root", root,
                           "--runtime", args.runtime.resolve(), "--clang", args.clang,
                           "--opt-level", opt, "--no-cache", "-o", root / case]
                if args.sanitize:
                    command.append("--sanitize")
                result = run(*command, cwd=root)
                if result.returncode:
                    sys.stderr.buffer.write(result.stderr)
                    return 1
                expected = run(args.python, "-E", root / (case + ".py"), cwd=root)
                actual = run(root / case, cwd=root)
                stdout = b"prior initialized\n" if case == "close-spelling" else b""
                assert expected.returncode == actual.returncode == 1 and expected.stdout == actual.stdout == stdout
                assert actual.stderr.startswith(b"ImportError: cannot import name ")
                if case == "close-spelling":
                    assert b"Did you mean: 'EDOM'?" in expected.stderr
                assert actual.stderr.splitlines() == expected.stderr.splitlines()[-1:], (expected.stderr, actual.stderr)
            print(f"O{opt}: {len(tests)}/{len(tests)} original methods, 2 failure controls, complete platform registry and three unused failing imports and caught error fields/optional hints matched")
            if args.evidence:
                reports.append(build_report(reference, int(opt), [(t, "complete module, fresh native process")
                                            for t in tests], observations))
            if args.result_dir:
                args.result_dir.mkdir(exist_ok=True, parents=True)
                report = {"cpython_commit": reference["cpython_commit"], "source_blob": digest,
                          "denominator": len(tests), "opt_level": opt, "platform": sys.platform,
                          "execution": "fresh native process per original method, direct method driver",
                          "tests": {t: {"status": "native"} for t in tests},
                          "controls": {"missing-code": "matched", "missing-attribute": "matched",
                                       "unknown-unused-import": "matched", "absent-target-unused-import": "matched",
                                       "close-spelling-import-hint": "matched", "optional-import-hints": "matched",
                                       "caught-import-fields": "matched"},
                          "registry_observations": len(oracle[("operations",)][1].splitlines())}
                (args.result_dir / f"native-{sys.platform}-O{opt}.json").write_text(json.dumps(report, indent=2) + "\n")
    if args.evidence:
        validate_pair(reports, reference)
        args.evidence.write_text(json.dumps(reports, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
