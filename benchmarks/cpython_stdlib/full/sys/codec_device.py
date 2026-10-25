"""Run the unchanged Linux test_os device-encoding case with and without a terminal."""

import argparse
from contextlib import nullcontext
import hashlib
import json
import os
import pty
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

SHARED = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SHARED))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_in_fixtures

CASE = 'test.test_os.DeviceEncodingTests.test_device_encoding'
DRIVER = b'import codecs\nimport sys\nentry = codecs.lookup(sys.argv[1])\nprint(entry.name)\nprint(bool(entry))\n'
FILES = {"Lib/test/test_os.py": "555c85f4857c2d7e69dda2f390847c7f02123cb5",
         "LICENSE": "20cf39097c68baa17cc566b64e76d34ebf034044"}


def host(binary):
    import test.test_os as original_module

    verify_interpreter_tests({'files': FILES})
    calls = []

    def execute(case, original):
        def lookup(encoding):
            result = subprocess.run([binary, encoding], capture_output=True, text=True, timeout=20, check=True)
            name, truth = result.stdout.splitlines()
            if result.stderr or truth not in ('True', 'False'):
                raise AssertionError((truth, result.stderr))
            calls.append({'encoding_from_os': encoding, 'native_name': name, 'native_truth': truth == 'True'})
            # This unchanged assertion consumes only the truth of the returned codec.
            return truth == 'True'
        registry = patch.object(original_module, 'codecs', SimpleNamespace(lookup=lookup)) if binary else nullcontext()
        with registry:
            original()
        return 'pass'

    observed, failures, skipped = run_in_fixtures(load_suite('test.test_os'), {CASE: execute})
    if failures:
        raise AssertionError(failures)
    return {'isatty': os.isatty(0), 'calls': calls, 'observed': observed, 'skips': dict(skipped)}


def compare(toolchain, env):
    with tempfile.TemporaryDirectory() as tmp:
        binary = toolchain.build(DRIVER, Path(tmp))
        master, slave = pty.openpty()
        try:
            group = {}
            for fixture, fd in (("redirected", subprocess.DEVNULL), ("pty", slave)):
                group[fixture] = pair = {}
                for subject, extra in (("stock", []), ("mixed", [binary])):
                    run = subprocess.run([sys.executable, "-B", __file__, "--observe", *extra], stdin=fd,
                                         capture_output=True, text=True, timeout=45, env=env, check=True)
                    row = json.loads(run.stdout)
                    if run.stderr or row['isatty'] != (fixture == 'pty'):
                        raise AssertionError((row, run.stderr))
                    if fixture == 'pty':
                        expected = [{'encoding_from_os': 'UTF-8', 'native_name': 'utf-8', 'native_truth': True}] if extra else []
                        if row['observed'] != {CASE: ['pass']} or row['skips'] or row['calls'] != expected:
                            raise AssertionError(row)
                    elif list(row['skips']) != [CASE] or row['calls'] or row['observed']:
                        raise AssertionError(row)
                    pair[subject] = row
                if fixture == 'redirected' and pair['stock'] != pair['mixed']:
                    raise AssertionError(pair)
            return group
        finally:
            os.close(slave)
            os.close(master)


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        raise SystemExit("run with test-enabled CPython 3.14.5 on Linux")
    if sys.argv[1:2] == ["--observe"]:
        print(json.dumps(host(sys.argv[2] if len(sys.argv) == 3 else None)))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    env = {key: value for key, value in os.environ.items() if key not in ("PYTHONIOENCODING", "PYTHONUTF8")}
    env.update(PYTHONPATH=str(args.cpython_source.resolve() / "Lib"), LC_ALL="C.UTF-8", LANG="C.UTF-8")
    report = dict(cpython_commit="5607950ef232dad16d75c0cf53101d9649d89115", files=FILES,
                  case_id=CASE, occurrence=1, native_driver_sha256=hashlib.sha256(DRIVER).hexdigest(),
                  roles_for_pty_mixed_run={"body": "cpython", "assertions": "cpython", "fixture": "cpython",
                                           "os.device_encoding": "cpython", "codecs.lookup and truth": "imre"},
                  sys_ids_added=0, levels={})
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        report["levels"][str(level)] = compare(toolchain, env)
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
