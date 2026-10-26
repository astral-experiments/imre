"""Observe the actual kernel race exercised by the unchanged CPython test_send_signal_race2."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
NAME = "test.test_subprocess.POSIXProcessTestCase.test_send_signal_race2"
ACTOR = """import subprocess
import sys
p = subprocess.Popen(sys.argv[1:])
print(p.pid)
command = input()
while command != "quit":
    if command == "poll":
        code = p.poll()
        if code is None:
            print("None")
        else:
            print(code)
    elif command == "status":
        code = p.returncode
        if code is None:
            print("None")
        else:
            print(code)
    elif command == "clear":
        p.returncode = None
        print("cleared")
    elif command == "install":
        p.poll = POLL_LAMBDA
        print("installed")
    elif command == "restore":
        del p.poll
        print("restored")
    elif command == "signal":
        p.send_signal(int(input()))
        print("sent")
    elif command == "kill":
        p.kill()
        print("killed")
    command = input()
p.wait()
"""


def main():
    sys.path[:0] = [str(HERE), str(HERE.parents[1])]
    from attempt_bodies import SOURCE, fixture
    from signal_race_originals import Parent, hash_file
    from suite_inventory import load_suite, verify_files, verify_interpreter_tests
    from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures
    from test import test_subprocess as suite

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tc = Toolchain(
        args.imre.resolve(),
        args.runtime.resolve(),
        args.clang.resolve(),
        args.opt_level,
    )
    method = NAME.rsplit(".", 1)[-1]
    body = method_bytes(SOURCE, "POSIXProcessTestCase", method)
    source = SOURCE.read_text()
    (node,) = (
        n
        for n in ast.walk(ast.parse(source))
        if isinstance(n, ast.FunctionDef) and n.name == method
    )
    (callback,) = (n for n in ast.walk(node) if isinstance(n, ast.Lambda))
    callback_text = ast.get_source_segment(source, callback)
    assert callback_text == "lambda: None"
    output = {
        "method_sha256": hashlib.sha256(body).hexdigest(),
        "replacement": callback_text,
    }

    class Process(Parent):
        def poll(self):
            code = self.request("poll")
            return None if code == b"None" else int(code)

        @Parent.returncode.setter
        def returncode(self, value):
            assert value is None and self.request("clear") == b"cleared"
            self.events.append("clear")

        def kill(self):
            assert self.request("kill") == b"killed"
            self.events.append("kill")

        def send_signal(self, sig):
            super().send_signal(sig)
            self.events.append("signal")

        def __setattr__(self, key, value):
            if key == "poll":
                code = value.__code__
                assert (
                    code.co_firstlineno == callback.lineno
                    and code.co_name == "<lambda>"
                )
                assert (
                    code.co_argcount == 0 and not code.co_freevars and not code.co_names
                )
                assert (
                    code.co_consts == (None,)
                    and Path(code.co_filename).read_bytes() == SOURCE.read_bytes()
                )
                if self.transfer:
                    assert self.request("install") == b"installed"
                self.events.append("install")
            super().__setattr__(key, value)

        def __delattr__(self, key):
            if key == "poll":
                if self.transfer:
                    assert self.request("restore") == b"restored"
                self.events.append("restore")
            super().__delattr__(key)

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        binary = tc.build(ACTOR.replace("POLL_LAMBDA", callback_text).encode(), root)
        output["native_parent_sha256"] = hash_file(binary)
        for transfer in (True, False):
            label = "installed" if transfer else "host_only_control"
            logs = root / label
            logs.mkdir()

            def run(case, original):
                holders = []

                def launch(argv):
                    assert argv == [sys.executable, "-c", "exit(1)"] and not holders
                    process = Process(binary, logs, argv)
                    process.events = []
                    process.transfer = transfer
                    holders.append(process)
                    return process

                if transfer:
                    d = root / "body"
                    d.mkdir()
                    owner, preamble = fixture(case, d)
                    try:
                        execute = native_method(tc, d, SOURCE, owner, NAME, preamble)
                    except RuntimeError as exc:
                        output["body_attempt"] = {
                            "status": "blocked",
                            "reason": str(exc)
                            .strip()
                            .splitlines()[-1]
                            .replace(str(d), "<case>"),
                        }
                    else:
                        output["body_attempt"] = {
                            "status": "completed-needs-operation-review",
                            **execute(case, original),
                        }
                try:
                    with patch.object(
                        suite, "subprocess", SimpleNamespace(Popen=launch)
                    ):
                        original()
                finally:
                    for process in holders:
                        process.close()
                (process,) = holders
                assert process.events == [
                    "install",
                    "clear",
                    "signal",
                    "restore",
                    "kill",
                ]
                assert "poll" not in vars(process)
                lines = process.trace()
                child = process.pid
                kills = [
                    line
                    for line in lines
                    if re.match(r"(kill|pidfd_send_signal)\(", line)
                ]
                reaps = [
                    line
                    for line in lines
                    if line.startswith(f"wait4({child},")
                    and line.endswith(f"= {child}")
                ]
                no_child = [
                    line
                    for line in lines
                    if line.startswith(f"wait4({child},") and "ECHILD" in line
                ]
                assert (
                    len(reaps) == len(no_child) == 1
                    and "WEXITSTATUS(s) == 1" in reaps[0]
                )
                if transfer:
                    (kill,) = kills
                    assert (
                        kill.startswith(f"kill({child}, SIGTERM)")
                        and "= -1 ESRCH" in kill
                    )
                    assert (
                        lines.index(reaps[0])
                        < lines.index(kill)
                        < lines.index(no_child[0])
                    )
                else:
                    assert not kills and lines.index(reaps[0]) < lines.index(
                        no_child[0]
                    )
                (child_exec,) = (
                    line
                    for line in (logs / f"pid.{child}").read_text().splitlines()
                    if line.startswith(f'execve("{sys.executable}",')
                    and '"-c", "exit(1)"' in line
                    and line.endswith("= 0")
                )
                output[label] = {
                    "native_parent": process.parent,
                    "child": child,
                    "child_exec": child_exec,
                    "fixture_events": process.events,
                    "reap": reaps[0],
                    "signals": kills,
                    "restored_wait": no_child[0],
                }
                return output[label]

            with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
                _, failures, skips = run_in_fixtures(
                    load_suite(manifest["module"]), {NAME: run}
                )
            assert not failures and not skips, (failures, skips)
    result = dict(
        cpython_commit=manifest["commit"],
        opt_level=tc.level,
        id=NAME,
        toolchain={k: hash_file(getattr(tc, k)) for k in ("imre", "runtime", "clang")},
        **output,
    )
    args.result.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
