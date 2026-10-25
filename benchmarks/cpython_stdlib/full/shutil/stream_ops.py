"""Exercise native stream copying with the two open file descriptions from each original fixture."""

import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tree_ops
from suite_evidence import roles
from suite_inventory import cases, load_suite
from suite_native import run_in_fixtures

SOURCE = '''import shutil
import sys
with open("/imre-shutil-fixture/source-fd", "rb") as src:
    with open("/imre-shutil-fixture/destination-fd", "wb") as dst:
        print(src.tell())
        print(dst.tell())
        shutil.copyfileobj(src, dst, length=int(sys.argv[1]))
        print(src.closed)
        print(dst.closed)
        print(src.tell())
        print(dst.tell())
'''


def executors(module, tools, work, *, with_roles=False):
    exe = tools.build(SOURCE.encode(), work)
    shim = work / "streams.so"
    if sys.platform.startswith("linux"):
        subprocess.run([tools.clang, "-shared", "-fPIC", "-Wall", "-Werror",
                        tree_ops.HERE / "stream_fds.c", "-o", shim], check=True)

    def execute(case, original):
        if not shim.exists():
            return {"status": "blocked", "reason": "open file description transport requires a platform adapter"}
        calls = []
        def copyfileobj(src, dst, length=0):
            start = [src.tell(), dst.tell()]
            rfd, wfd = src.fileno(), dst.fileno()
            proc = subprocess.run([exe, str(length)], pass_fds=(rfd, wfd),
                env=dict(os.environ, LD_PRELOAD=str(shim), SHUTIL_FIXTURE_SRC=str(rfd), SHUTIL_FIXTURE_DST=str(wfd)),
                text=True, capture_output=True, timeout=60)
            assert proc.returncode == 0 and not proc.stderr, (proc.stdout, proc.stderr)
            a, b, src_closed, dst_closed, c, d = proc.stdout.splitlines()
            assert [int(a), int(b)] == start
            assert (src_closed, dst_closed) == ("False", "False")
            assert [int(c), int(d)] == [src.tell(), dst.tell()]
            calls.append({"start": start, "end": [int(c), int(d)], "both_open": True})
        with patch.object(module, "shutil", SimpleNamespace(copyfileobj=copyfileobj)):
            original()
        assert len(calls) == 1
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": dict(operation="shutil.copyfileobj", native_calls=1, opt_level=tools.level,
                                     fixture="original CPython get_files context; native handles share both open file descriptions",
                                     **calls[0])}
        return {"status": "host_driven_native", "native_calls": 1,
                "fixture": "original open file descriptions shared by dup", **calls[0]}
    suite = load_suite("test.test_shutil")
    return {case.id(): execute for case in cases(suite) if isinstance(case, module.TestCopyFileObj)}


def execute_level(module, tools, work):
    suite = load_suite("test.test_shutil")
    executions = executors(module, tools, work)
    evidence, failures, skips = run_in_fixtures(suite, executions)
    rows = {name: values[0] for name, values in evidence.items() if values}
    rows.update({name: {"status": "failed", "reason": m} for name, m in failures})
    rows.update({name: {"status": "reference_skip", "reason": m} for name, m in skips})
    assert rows.keys() == executions.keys()
    return rows


if __name__ == "__main__":
    sys.exit(tree_ops.main(execute_level))
