"""Run the original generated bodies in Imre under CPython 3.14.5 fixtures.

From the Imre checkout with the pinned, test-enabled interpreter on Linux:

    python3.14 -B benchmarks/cpython_stdlib/full/argparse/generated_bodies.py \
        --imre target/debug/imre --runtime target/debug/libimre_runtime.a \
        --clang clang-18 --result /tmp/argparse-generated-bodies.json

Each result file contains separate -O0 and -O3 observations.
"""

import argparse
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import unittest

from check_registration import HERE, stage_module, target_ids

sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files
from suite_native import run_in_fixtures


# Only the parent fixture runner calls setUp / tearDown. The native result
# runner attaches to the case so that subTest reports every failure or skip.
ENTRY = '''import json
import os
import sys
import unittest
import test.test_argparse as suite

def host_fixture():
    pass

failed = False
with open("/dev/stdin", "rb") as commands:
    while True:
        line = commands.readline()
        if not line:
            break
        name = line.decode().strip()
        owner, method = name.rsplit(".", 2)[-2:]
        case = getattr(suite, owner)()
        case._testMethodName = method
        assert case.id() == name
        assert os.environ.get("COLUMNS") == "80"
        runner = unittest._MainRunner()
        runner.verbose = 0
        runner._run(case, name, host_fixture, getattr(case, method), host_fixture)
        assert runner.total == 1
        failed = failed or bool(runner.failures or runner.errors or runner.skips)
        print(json.dumps([name, runner.failures, runner.errors, runner.skips]), flush=True)
if failed:
    sys.exit(1)
'''


def measure(options, names, level, work):
    from test import test_argparse as original

    binary = work / f"generated-O{level}"
    subprocess.run([options.imre, "build", work / "case.py", "-o", binary,
                    "--runtime-imports", "--opt-level", str(level),
                    "--runtime", options.runtime, "--clang", options.clang], check=True)
    # A waiting child runs no case between callbacks. Only reuse its environment
    # when the complete fixture environment and current working directory match.
    baseline = dict(os.environ)
    cwd = os.getcwd()
    environment = {**baseline, "COLUMNS": "80"}
    suite = load_suite("test.test_argparse")
    records = {}
    child = None
    with tempfile.TemporaryFile("w+b") as errors:
        def execute(case, _):
            nonlocal child
            if (type(case).setUp is not original.TestCase.setUp
                    or type(case).tearDown is not unittest.TestCase.tearDown
                    or dict(os.environ) != environment or os.getcwd() != cwd):
                raise ValueError("unexpected host fixture state for " + case.id())
            if child is None:
                child = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         stderr=errors, text=True, encoding="utf-8")
            child.stdin.write(case.id() + "\n")
            child.stdin.flush()
            if not select.select([child.stdout], [], [], 120)[0]:
                child.kill()
                raise TimeoutError(case.id())
            line = child.stdout.readline()
            if not line:
                errors.seek(0)
                raise RuntimeError("native program stopped: " + errors.read().decode(errors="replace")[-4000:])
            case_id, failures, exceptions, skips = json.loads(line)
            if case_id != case.id():
                raise ValueError("native occurrence mismatch: " + case_id)
            records[case_id] = dict(failures=failures, errors=exceptions, skips=skips)
            if len(records) % 100 == 0:
                print(f"-O{level} reached {len(records)} of {len(names)}", flush=True)
            if failures or exceptions:
                raise AssertionError(f"native result for {case_id}: {records[case_id]}")
            if skips:
                raise unittest.SkipTest(f"native runner reported {skips} skips for {case_id}")
            return case_id

        try:
            completed, failures, skips = run_in_fixtures(suite, dict.fromkeys(names, execute))
        finally:
            if child is not None:
                child.stdin.close()
                status = child.wait(timeout=30)
            else:
                status = None
        errors.seek(0)
        stderr = errors.read().decode(errors="replace")
    if dict(os.environ) != baseline or os.getcwd() != cwd:
        raise RuntimeError("host fixtures did not restore the original environment")
    if stderr or status != int(any(r["failures"] or r["errors"] or r["skips"] for r in records.values())):
        raise RuntimeError(f"native program exit {status}, stderr={stderr!r}")
    rejected = {name for name, _ in failures + skips}
    if rejected - names:
        raise RuntimeError(f"class or module fixture failure: {failures!r} {skips!r}")
    passed = set(completed) - rejected
    if passed | rejected != names or len(records) != len(names):
        raise RuntimeError("missing per-occurrence results")
    return dict(opt_level=level, selected=len(names), completed=sorted(passed),
                fixture="CPython", body="Imre", operation="Imre", assertions="Imre",
                failures=failures, skips=skips, native_results=records,
                exit_status=status, stderr=stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--class-name", action="append", help="restrict to complete target classes")
    parser.add_argument("--result", required=True, type=Path)
    options = parser.parse_args()
    if sys.platform != "linux" or sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("use a test-enabled CPython 3.14.5 on Linux")
    import test.test_argparse as upstream
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    shared = json.loads((HERE.parent / "shared/UPSTREAM.json").read_bytes())
    helper = "Lib/test/support/os_helper.py"
    verify_files(Path(upstream.os_helper.__file__).parent, {"os_helper.py": shared["files"][helper]})
    verify_files(Path(upstream.__file__).parent, {"test_argparse.py": manifest["files"]["Lib/test/test_argparse.py"]})
    names = target_ids()
    if options.class_name:
        names = {n for n in names if n.rsplit(".", 2)[1] in options.class_name}
    classes = {n.rsplit(".", 2)[1] for n in names}
    if options.class_name and classes != set(options.class_name):
        parser.error("unknown target class")
    discovered = Counter(case.id() for case in cases(load_suite(manifest["module"])))
    if any(discovered[name] != 1 for name in names):
        parser.error("target multiplicity changed")
    options.imre = options.imre.resolve()
    options.runtime = options.runtime.resolve()
    with tempfile.TemporaryDirectory(prefix="imre-argparse-bodies-") as temp:
        work = Path(temp)
        # Concurrent rebuilds can replace shared artifacts. Both Clang
        # invocations must link exactly the same runtime.
        for name in ("imre", "runtime"):
            saved = work / ("imre" if name == "imre" else "libimre_runtime.a")
            shutil.copy2(getattr(options, name), saved)
            setattr(options, name, saved)
        digests = {}
        for name in ("imre", "runtime"):
            with getattr(options, name).open("rb") as artifact:
                digests[name] = hashlib.file_digest(artifact, "sha256").hexdigest()
        stage_module(work, classes, bodies=True)
        (work / "case.py").write_text(ENTRY)
        with ProcessPoolExecutor(max_workers=2) as pool:
            jobs = [pool.submit(measure, options, names, level, work) for level in (0, 3)]
            results = [job.result() for job in jobs]
    for result in results:
        result["toolchain_sha256"] = digests
    options.result.write_text(json.dumps(results, indent=2) + "\n")
    for result in results:
        print(f"-O{result['opt_level']} passed {len(result['completed'])} of {len(names)} with host fixtures")
    return int(any(result["failures"] or result["skips"] for result in results))


if __name__ == "__main__":
    sys.exit(main())
