"""Compile unchanged Luhn and validate upstream tests, Unicode, and error recovery."""

import argparse
import json
import os
import runpy
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
validator = SimpleNamespace(**runpy.run_path(str(ROOT / "benchmarks/luhn_validate.py")))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    records = []
    with tempfile.TemporaryDirectory(prefix="relar-luhn-suite-") as temporary:
        directory = Path(temporary)
        for level in args.opt_levels:
            programs = {}
            for name in ("driver", "checks"):
                binary = directory / f"{name}-O{level}"
                command = [
                    str(args.compiler.resolve()),
                    "build",
                    str(validator.PACKAGE / f"{name}.py"),
                    "--module-root",
                    str(validator.PACKAGE),
                    "-o",
                    str(binary),
                    "--opt-level",
                    level,
                    *flags,
                ]
                built = subprocess.run(
                    command, capture_output=True, timeout=120, check=False
                )
                assert built.returncode == 0 and not built.stderr, (
                    command,
                    built.stderr,
                )
                programs[name] = binary
            check_command = validator.python_command(args.python)
            check_command[-1] = str(validator.PACKAGE / "checks.py")
            expected = subprocess.run(
                check_command, capture_output=True, timeout=60, check=False
            )
            actual = subprocess.run(
                [str(programs["checks"])],
                capture_output=True,
                timeout=60,
                check=False,
                env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
            )
            assert expected.returncode == actual.returncode == 0, (
                expected.stderr,
                actual.stderr,
            )
            assert not expected.stderr and not actual.stderr
            assert expected.stdout == actual.stdout
            output = directory / f"validation-O{level}.json"
            checked = subprocess.run(
                [
                    args.python,
                    "-I",
                    "-B",
                    str(ROOT / "benchmarks/luhn_validate.py"),
                    "--binary",
                    str(programs["driver"]),
                    "--output",
                    str(output),
                ],
                capture_output=True,
                text=True,
                timeout=240,
                check=False,
            )
            assert checked.returncode == 0, (checked.stdout, checked.stderr)
            record = json.loads(output.read_text())
            record.update(
                {"opt_level": level, "sanitize": args.sanitize, "recovery": "passed"}
            )
            records.append(record)
            print(f"O{level}: {checked.stdout.strip()}")
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(records, indent=2) + "\n")


if __name__ == "__main__":
    main()
