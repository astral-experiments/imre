"""Compare Ruff's complete, pinned CLI on CPython and a Imre executable."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / "examples/conformance_report/upstream"
SOURCE = UPSTREAM / "conformance.py"
ANALYZER = ROOT / "tests/fixtures/ruff_program/analyzer.py"
SENTINEL = b"previous report\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--source", type=Path, default=SOURCE, help="Ruff script used to build the executable")
    args = parser.parse_args()
    if sys.flags.optimize:
        raise RuntimeError("validation requires Python assertions")
    source = args.source.resolve()
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    if source == SOURCE:
        assert digest == json.loads((UPSTREAM / "UPSTREAM.json").read_bytes())["files"][SOURCE.name]
    commands = [[args.python, "-s", "-B", str(source)], [str(args.binary.resolve())]]
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        suite = root / "typing/conformance"
        (suite / "src").mkdir(parents=True)
        (suite / "tests").mkdir()
        groups = suite / "src/test_groups.toml"
        groups.write_text('[cases]\nname = "Cases"\n', encoding="utf-8")
        (suite / "tests/cases_one.py").write_bytes('x = "🦀"  # E: example\r\n'.encode())
        (suite / "tests/cases_types.pyi").write_text("y: str  # E\n", encoding="utf-8")
        (suite / "tests/support.py").write_text("# ignored\n", encoding="utf-8")
        report = root / "report.md"
        environment = {
            **os.environ, "PYTHONHASHSEED": "0", "COLUMNS": "80", "NO_COLOR": "1",
            "PYTHON_COLORS": "0", "PYTHONDONTWRITEBYTECODE": "1",
        }
        environment.pop("CONFORMANCE_SUITE_COMMIT", None)
        environment.pop("IMRE_EXPECTED_PYTHON_VERSION", None)

        def compare(flags, *, status=0, file=False, error=None):
            results = []
            for command in commands:
                report.write_bytes(SENTINEL)
                run = subprocess.run([*command, *flags], cwd=root, env=environment,
                                     capture_output=True, timeout=45)
                assert run.returncode == status, (command, flags, run.stderr)
                stderr = run.stderr
                if error:
                    # Python includes its traceback. Compare the entire exception payload.
                    marker = (error + ": ").encode()
                    assert marker in stderr, (command, flags, stderr)
                    stderr = stderr.rsplit(marker, 1)[1]
                results.append((run.stdout, stderr, report.read_bytes()))
            assert results[0] == results[1], (flags, results)
            stdout, stderr, contents = results[0]
            if file:
                assert not stdout and contents != SENTINEL
                assert stderr == b"Output written to report.md\n" + contents + b"\n"
            else:
                assert contents == SENTINEL
            return stdout

        def analyzers(old, new):
            return ["--old-ty", args.python, str(ANALYZER), old,
                    "--new-ty", args.python, str(ANALYZER), new]

        compare(["--help"])
        compare([], status=2)
        compare([*analyzers("same", "changed"), "--format=unknown"], status=2)
        compact = compare(analyzers("same", "same"))
        assert b"No changes detected" in compact
        environment["IMRE_EXPECTED_PYTHON_VERSION"] = "3.13"
        compare([*analyzers("same", "changed"), "--python-version=3.13"])
        del environment["IMRE_EXPECTED_PYTHON_VERSION"]
        environment["CONFORMANCE_SUITE_COMMIT"] = "example-revision"
        for format in ("diff", "github"):
            flags = [*analyzers("same", "changed"), "--tests-path", "typing/conformance",
                     "--format", format, "--force-summary-table"]
            stdout = compare(flags)
            assert stdout.endswith(b"\n")
            if format == "github":
                assert b"example-revision" in stdout
            compare([*flags, "--output", "report.md"], file=True)
        for mode, exception in (("empty", "AssertionError"), ("exit", "CalledProcessError"),
                                ("invalid", "JSONDecodeError")):
            compare([*analyzers("same", mode), "--output", "report.md"], status=1, error=exception)
        groups.write_bytes(b"[invalid\n")
        compare(analyzers("same", "same"), status=1, error="TOMLDecodeError")
    print("Ruff's original CLI matched CPython on 13 cases")


if __name__ == "__main__":
    main()
