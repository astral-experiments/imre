"""Run the original OS seek assertions on the descriptor used by the CPython fixture."""

import argparse
from contextlib import chdir
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

STDLIB = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(STDLIB), str(STDLIB / "full/os")]
from native_descriptors import verify_sources
from suite_inventory import load_suite
from suite_native import Toolchain, run_in_fixtures

DRIVER = b'''import os
import sys

try:
    print(os.lseek(int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])))
except OSError as error:
    print("OSError: " + str(error))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    modules, commit = verify_sources(args.cpython_source)
    tool = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    result = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        binary = tool.build(DRIVER, work)
        for key, short, inherit in (("test_os", "TestInvalidFD.test_lseek", False),
                                    ("test_posix", "PosixTester.test_fs_holes", True)):
            original = modules[key]
            case_id = original.__name__ + "." + short
            def execute(case, method):
                calls = []
                def seek(fd, pos, whence):
                    # The open descriptor shares its kernel offset through exec.
                    # The invalid-descriptor method intentionally transfers no open fd.
                    p = subprocess.run([binary, str(fd), str(pos), str(whence)], cwd=work,
                                       pass_fds=(fd,) if inherit else (), capture_output=True,
                                       text=True, check=True, timeout=30)
                    if p.stderr:
                        raise AssertionError(p.stderr)
                    if error := re.fullmatch(r"OSError: \[Errno (\d+)\] (.+)\n", p.stdout):
                        number = int(error[1])
                        calls.append({"position": pos, "whence": whence, "errno": number})
                        raise OSError(number, error[2])
                    offset = int(p.stdout)
                    calls.append({"position": pos, "whence": whence, "result": offset})
                    return offset
                view = SimpleNamespace(lseek=seek)
                if inherit:
                    view.SEEK_DATA = original.os.SEEK_DATA
                    view.SEEK_HOLE = original.os.SEEK_HOLE
                with patch.object(original, "os", view):
                    method()
                if not calls:
                    raise AssertionError(f"{case.id()} made no native seek call")
                return {"roles": {"body": "cpython", "assertions": "cpython", "fixture": "cpython",
                                  "operation": "imre"}, "native_lseek": calls}
            with chdir(work):
                observed, failures, skips = run_in_fixtures(load_suite(original.__name__), {case_id: execute})
            if failures or skips or set(observed) != {case_id}:
                raise AssertionError((failures, skips, observed))
            result[case_id], = observed[case_id]
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": args.opt_level,
                                       "cases": result}, indent=2) + "\n")
    print(f"O{args.opt_level}: two original OS bodies and assertions, native seeks; no logging occurrences")


if __name__ == "__main__":
    main()
