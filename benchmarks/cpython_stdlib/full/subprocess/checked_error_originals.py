"""Trace the real validation, exit status and timeout in nine complete native methods."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from attempt_bodies import SOURCE, fixture
from selector_fixture import selected_entry, verify_binding
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

HERE = Path(__file__).resolve().parent
METHODS = {
    "test_check_call_nonzero",
    "test_check_output_nonzero",
    "test_check_output_timeout",
    "test_invalid_cmd",
}
IDS = {
    f"test.test_subprocess.{cls}.{m}"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
    for m in METHODS
} | {"test.test_subprocess.RunFuncTestCase.test_check_zero"}


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def measure(tc, root, case, original):
    from test import test_subprocess as upstream

    no_poll = type(case).__name__.endswith("NoPoll")
    verify_binding(upstream.subprocess, no_poll)
    method = case._testMethodName
    d = root / case.id()
    d.mkdir()
    binary = d / "native"
    ir = d / "case.ll"

    class Observed:
        level = tc.level

        def build(self, source, directory):
            entry = directory / "case.py"
            source = selected_entry(source) if no_poll else source
            entry.write_bytes(source)
            built = subprocess.run(
                [
                    tc.imre,
                    "build",
                    entry,
                    "-o",
                    binary,
                    "--no-cache",
                    "--opt-level",
                    str(tc.level),
                    "--runtime",
                    tc.runtime,
                    "--clang",
                    tc.clang,
                    "--emit-llvm",
                    ir,
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            if built.returncode:
                raise AssertionError(built.stderr)
            wrapper = directory / "trace"
            wrapper.write_text(
                "#!/bin/sh\nexec "
                + shlex.join(
                    [
                        "strace",
                        "-ff",
                        "-qq",
                        "-s",
                        "768",
                        "-e",
                        "signal=none",
                        "-e",
                        "trace=execve,clone,clone3,vfork,fork,wait4,waitid,kill,poll,ppoll,select,pselect6",
                        "-o",
                        str(d / "pid"),
                        "--",
                        str(binary),
                    ]
                )
                + ' "$@"\n'
            )
            wrapper.chmod(0o755)
            return wrapper

    owner, pre = fixture(case, d)
    native = native_method(
        Observed(), d, SOURCE, owner, case.id(), pre, operation=("imre", method)
    )
    body = native(case, original)
    traces = {
        int(p.name.rsplit(".", 1)[1]): p.read_text().splitlines()
        for p in d.glob("pid.*")
    }
    [parent] = [
        pid
        for pid, lines in traces.items()
        if any(
            s.startswith("execve(" + json.dumps(str(binary)) + ",")
            and s.endswith("= 0")
            for s in lines
        )
    ]
    lines = traces[parent]
    launches = [
        int(m[1])
        for s in lines
        if (m := re.match(r"(?:clone3?|fork|vfork)\(.*= (\d+)$", s))
    ]
    runtime_calls = re.findall(
        r"\bcall (?:ptr|i64|void) @(imre_(?:popen|call|run|check_output)[a-z_]*)\(",
        ir.read_text(),
    )
    product = [
        s
        for s in runtime_calls
        if s
        in {
            "imre_popen_config",
            "imre_call_config",
            "imre_run_config",
            "imre_popen_redirects_config",
            "imre_popen_extras",
        }
    ]
    ops = dict(
        native_parent_sha256=sha(binary),
        llvm_sha256=sha(ir),
        runtime_calls=product,
        data_child=None,
    )
    selectors = [
        s.split("(", 1)[0]
        for s in lines
        if re.match(r"(poll|ppoll|select|pselect6)\(", s)
    ]
    if method == "test_invalid_cmd":
        assert not launches and product.count("imre_popen_config") == 2, (
            launches,
            runtime_calls,
        )
        ops.update(nul_rejections=2, launched=0)
    else:
        [pid] = launches
        child = traces[pid]
        wanted = {
            "test_check_call_nonzero": ("imre_call_config", "sys.exit(47)", 47),
            "test_check_output_nonzero": ("imre_run_config", "sys.exit(5)", 5),
            "test_check_output_timeout": (
                "imre_run_config",
                "time.sleep(3600)",
                -9,
            ),
        }
        if method == "test_check_zero":
            api, code = "imre_run_config", 0
            command = list(upstream.ZERO_RETURN_CMD)
            assert command and all(isinstance(arg, str) for arg in command)
            executable, program = command[0], command[-1]
        else:
            api, program, code = wanted[method]
            executable = sys.executable
        assert api in product, (method, runtime_calls)
        if method == "test_check_zero":
            module = ir.read_text()
            call = re.search(
                r"call ptr @imre_run_config\([^\n]+i1 (true|1|%\w+), ptr @layout_completed_process_[^)]+\)",
                module,
            )
            assert call, "checked run must allocate a CompletedProcess"
            if call[1].startswith("%"):
                before = module[: call.start()].rsplit("\ndefine ", 1)[-1]
                load = re.search(rf"{re.escape(call[1])} = load i1, ptr (%\w+)", before)
                assert load, "checked flag must come from the captured keyword"
                stores = re.findall(
                    rf"store i1 (\w+), ptr {re.escape(load[1])}\b",
                    before[: load.start()],
                )
                assert stores and stores[-1] in ("1", "true"), stores
        elif api == "imre_run_config":
            assert re.search(
                r"call ptr @imre_run_config\([^\n]+i1 true, ptr null\)", ir.read_text()
            ), "checked output requires check and the bare capture result"
        assert any(
            s.startswith("execve(" + json.dumps(executable) + ",")
            and program in s
            and s.endswith("= 0")
            for s in child
        ), child[:3]
        reaped = [
            s for s in lines if s.startswith(f"wait4({pid},") and s.endswith(f"= {pid}")
        ]
        assert len(reaped) == 1, reaped
        if code < 0:
            assert "WTERMSIG(s) == SIGKILL" in reaped[0] and any(
                s.startswith(f"kill({pid}, SIGKILL)") and s.endswith("= 0")
                for s in lines
            )
            assert selectors and set(selectors) <= (
                {"select", "pselect6"} if no_poll else {"poll", "ppoll"}
            ), selectors
        else:
            assert f"WEXITSTATUS(s) == {code}" in reaped[0]
        ops.update(
            launched=1,
            data_child=("original fixture ZERO_RETURN_CMD: " + repr(command))
            if method == "test_check_zero"
            else "pinned CPython 3.14.5: " + program,
            returncode=code,
            native_reaps=1,
        )
    ops.update(native_selector_syscalls=sorted(set(selectors)))
    return dict(
        body=body,
        roles=dict(body="imre", assertions="imre", operation="imre", fixture="cpython"),
        observed=ops,
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--opt-level", type=int, required=True, choices=(0, 3))
    a = p.parse_args()
    man = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", man["files"])
    verify_interpreter_tests(man)
    tc = Toolchain(a.imre, a.runtime, a.clang, a.opt_level)
    with (
        tempfile.TemporaryDirectory() as tmp,
        patch.dict(os.environ, IMRE_PYTHON=sys.executable),
    ):

        def execute(case, original):
            return measure(tc, Path(tmp), case, original)

        rows, fail, skips = run_in_fixtures(
            load_suite(man["module"]), dict.fromkeys(IDS, execute)
        )
        if fail or skips or rows.keys() != IDS:
            raise AssertionError((fail, skips))
    a.result.write_text(
        json.dumps(
            dict(
                cpython_commit=man["commit"],
                opt_level=tc.level,
                toolchain={k: sha(getattr(a, k)) for k in ("imre", "runtime", "clang")},
                rows=rows,
            ),
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
