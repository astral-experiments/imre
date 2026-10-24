"""Stand in for either ty command and check the arguments Ruff supplied."""

import json
import os
from pathlib import Path
import sys

mode, command, *flags = sys.argv[1:]
assert command == "check"
assert flags[0] == "--python-version=" + os.environ.get("IMRE_EXPECTED_PYTHON_VERSION", "3.12")
assert flags[1] == "--output-format=gitlab"
paths = [Path(arg) for arg in flags[flags.index("--exit-zero") + 2 :]]
assert [path.name for path in paths] == ["cases_one.py", "cases_types.pyi"]
assert all(path.is_absolute() for path in paths)
assert flags[flags.index("--exit-zero") + 1] == f"--extra-search-path={paths[0].parent}"
if mode == "exit":
    sys.exit(7)
if mode == "invalid":
    print("[{")
    sys.exit()
if mode == "empty":
    print("[]")
    sys.exit()

def diagnostic(path, line):
    point = {"line": line, "column": 1}
    return {
        "severity": "major",
        "check_name": "example",
        "description": "example: 🦀",
        "location": {"path": str(path), "positions": {"begin": point, "end": point}},
    }

records = [{"severity": "minor"}, diagnostic(paths[0], 1)]
if mode == "changed":
    records.append(diagnostic(paths[1], 1))
print(json.dumps(records))
