"""Run the untouched test_ioencoding against Imre's actual -c command."""
import hashlib
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import run_in_fixtures

CASE = "test.test_sys.SysModuleTest.test_ioencoding"


def run_subject(module, executable, flags=()):
    children = []

    class Child(subprocess.Popen):
        def __init__(self, argv, **kwargs):
            if len(argv) != 3 or argv[:2] != [executable, "-c"]:
                raise ValueError(f"unexpected test_ioencoding invocation: {argv!r}")
            env = kwargs["env"]
            if flags:
                env = dict(env, IMRE_ENCODED_FLAGS="\x1f".join(flags))
            self.record = {"source": argv[2], "source_sha256": hashlib.sha256(argv[2].encode()).hexdigest(),
                           "PYTHONIOENCODING": env["PYTHONIOENCODING"]}
            children.append(self.record)
            super().__init__(argv, **(kwargs | {"env": env}))

        def communicate(self, *args, **kwargs):
            out, err = super().communicate(*args, **kwargs)
            self.record.update(returncode=self.returncode, stdout_hex=out.hex(),
                               stderr_hex=None if err is None else err.hex())
            return out, err

    def execute(case, original):
        with (patch.object(module, "sys", SimpleNamespace(executable=executable)),
              patch.object(module, "subprocess", SimpleNamespace(PIPE=subprocess.PIPE, Popen=Child))):
            try:
                original()
            except AssertionError as exc:
                return "failure", str(exc)
        return "pass", "all original stdout and stderr comparisons passed"

    results, failures, skips = run_in_fixtures(load_suite("test.test_sys"), {CASE: execute})
    if failures or skips:
        raise AssertionError((failures, skips))
    (outcome, reason), = results[CASE]
    if not children or any("returncode" not in row for row in children):
        raise AssertionError(children)
    return outcome, reason, children


def observe(module, toolchain, method_sha256):
    stock, detail, control = run_subject(module, sys.executable)
    if stock != "pass" or len(control) != 5:
        raise AssertionError((stock, detail, control))
    flags = ["--quiet", "--no-cache", "--runtime", str(toolchain.runtime.resolve()),
             "--clang", str(toolchain.clang.resolve()), "--opt-level", str(toolchain.level)]
    outcome, reason, children = run_subject(module, str(toolchain.imre.resolve()), flags)
    return {"outcome": outcome, "reason": reason,
            "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
            "evidence": {"operation": "original native -c children select and write stdout",
                         "assertions": "unchanged CPython output and encoding error comparisons",
                         "fixture": "CPython decorators, SysModuleTest hooks, environment and subprocess pipes",
                         "method_sha256": method_sha256, "opt_level": toolchain.level, "children": children,
                         "stock_cpython_control": {"outcome": stock, "children": control}}}
