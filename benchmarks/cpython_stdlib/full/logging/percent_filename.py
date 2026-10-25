"""Exercise BaseFileTest.assertLogFile unchanged, including its failure message."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import verify_files
from suite_native import Toolchain, method_bytes

SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
FIXTURE = b"""import os
import unittest

class Case(unittest.TestCase):
    rmfiles: list[str]

    def __init__(self) -> None:
        super().__init__()
        self.rmfiles = []
"""
CALL = '''
case = Case()
case.assertLogFile("visible.log")
print(repr(case.rmfiles))
try:
    case.assertLogFile("absent 'π'.log")
except AssertionError as error:
    print(str(error))
else:
    raise AssertionError("missing file did not raise")
print(repr(case.rmfiles))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("requires test-enabled CPython 3.14.5 with assertions")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    body = method_bytes(SOURCE, "BaseFileTest", "assertLogFile")
    entry = FIXTURE + b"\n" + body + CALL.encode()
    report = {"cpython_commit": manifest["commit"], "method": "BaseFileTest.assertLogFile",
              "method_sha256": hashlib.sha256(body).hexdigest(), "logging_occurrences": 0, "levels": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        (work / "visible.log").touch()
        source = work / "reference.py"
        source.write_bytes(entry)
        def output(command):
            child = subprocess.run(command, cwd=work, capture_output=True, check=True, timeout=30)
            if child.stderr:
                raise AssertionError(child.stderr)
            return child.stdout.decode().splitlines()
        expected = output([sys.executable, "-I", "-B", source])
        for level in (0, 3):
            tool = Toolchain(args.imre, args.runtime, args.clang, level)
            actual = output([tool.build(entry, work)])
            if actual != expected:
                raise AssertionError((level, expected, actual))
            report["levels"][str(level)] = actual
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("BaseFileTest.assertLogFile: existing and missing filenames match CPython at O0 and O3; zero test IDs")


if __name__ == "__main__":
    main()
