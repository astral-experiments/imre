"""Compare intact JSON startup against both pinned fixture trees."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "cpython-git", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    runner = Path(__file__).resolve().parents[1] / "check_suite_startup.py"
    command = [sys.executable, runner, "--compiler", args.compiler]
    root = re.escape(tempfile.gettempdir()) + r"/imre-cpython-suite-[^/;]+/Lib(?=;)"
    reports = []
    for source, options in (("vendored", []), ("git", ["--cpython-git", args.cpython_git])):
        proc = subprocess.run([*command, *options, "test.test_json"], capture_output=True)
        if proc.returncode not in (0, 1) or proc.stderr:
            raise RuntimeError((proc.returncode, proc.stderr))
        report = json.loads(proc.stdout)
        checks = report["checks"]
        expected = {(entry, reachable) for entry in ("import", "runner") for reachable in (False, True)}
        if (report.pop("source") != source or len(checks) != 4
                or {(row["entry"], row["reachable_only"]) for row in checks} != expected):
            raise ValueError("incomplete JSON startup report")
        for row in checks:
            row["diagnostic"] = re.sub(root, "<staged>/Lib", row["diagnostic"])
        reports.append(report)
    if reports[0] != reports[1]:
        raise ValueError(f"vendored and full pinned startup differ: {reports}")
    args.result.write_text(json.dumps({**reports[0], "sources": ["vendored", "git"]}, indent=2) + "\n")


if __name__ == "__main__":
    main()
