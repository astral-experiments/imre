"""Drive the unchanged test_exit through native calls and exact child sources."""
import ast
import hashlib
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import NativeCompileError, method_bytes, run_in_fixtures


def exit_observation(module, toolchain, directory, source):
    from test.support import script_helper

    test_id = "test.test_sys.SysModuleTest.test_exit"
    # script_helper probes the CPython fixture executable once to choose its
    # bootstrap flags. That probe is not an execution of the module operation.
    script_helper.interpreter_requires_environment()
    events = []
    children = []

    def build(program, kind):
        path = directory / str(len(events))
        path.mkdir()
        data = program.encode()
        event = {"kind": kind, "source_sha256": hashlib.sha256(data).hexdigest()}
        events.append(event)
        try:
            return toolchain.build(data, path), event
        except NativeCompileError as exc:
            event["diagnostic"] = str(exc).replace(str(path), "[case]").strip()
            raise

    def exit_call(*values):
        call = "sys.exit(" + ", ".join(repr(value) for value in values) + ")"
        remark = "  # ty: ignore[too-many-positional-arguments]" if len(values) > 1 else ""
        program = (
            "import sys\ntry:\n    " + call + remark + "\n"
            'except SystemExit as error:\n    print("SystemExit")\n    print(repr(error.code))\n'
            'except TypeError as error:\n    print("TypeError")\n    print(repr(str(error)))\n'
        )
        binary, event = build(program, "caught sys.exit")
        run = subprocess.run([binary], check=True, capture_output=True, text=True, timeout=15)
        if run.stderr:
            raise AssertionError(run.stderr)
        name, literal = run.stdout.splitlines()
        event.update(call=call, exception=name, payload=literal)
        # Only reconstruct a verified native exception for the original host assertion.
        payload = ast.literal_eval(literal)
        if name == "TypeError":
            raise TypeError(payload)
        if name == "SystemExit":
            raise SystemExit(payload)
        raise AssertionError(f"unrecognized native exit result: {run.stdout!r}")

    def popen(argv, **kwargs):
        if (argv[0] != sys.executable or argv[-2] != "-c"
                or argv[1:-2] not in (["-X", "faulthandler", "-I"],
                                       ["-X", "faulthandler", "-E"], ["-X", "faulthandler"])):
            raise AssertionError(f"unexpected upstream launcher arguments: {argv!r}")
        program = argv[-1]
        try:
            binary, event = build(program, "original child")
        finally:
            events[-1]["source"] = program
        # Keep the upstream environment, pipes, cwd and child exit status.
        proc = subprocess.Popen([binary], **kwargs)
        children.append((event, proc))
        return proc

    def execute(case, original):
        child_fixture = SimpleNamespace(PIPE=subprocess.PIPE, Popen=popen, _cleanup=subprocess._cleanup)
        with (patch.object(module, "sys", SimpleNamespace(exit=exit_call)),
              patch.object(script_helper, "subprocess", child_fixture)):
            try:
                original()
            except NativeCompileError:
                if "diagnostic" not in events[-1]:
                    raise
                return "blocked", events[-1]["diagnostic"].splitlines()[-1]
            except AssertionError as exc:
                return "failure", str(exc).replace(str(directory), "[case]")
        raise AssertionError("test_exit completed: register its passing adapter in the suite selection")

    results, failures, skips = run_in_fixtures(load_suite("test.test_sys"), {test_id: execute})
    if failures or skips:
        raise AssertionError((failures, skips))
    (outcome, reason), = results[test_id]
    for event, proc in children:
        if proc.returncode is None:
            raise AssertionError("upstream child did not finish")
        event["returncode"] = proc.returncode
    completed = sum("returncode" in event or "exception" in event for event in events)
    return {
        "outcome": outcome, "reason": reason,
        "roles": roles(body="cpython", assertions="cpython",
                       operation="imre" if completed else "unobserved", fixture="cpython"),
        "evidence": {
            "operation": "native sys.exit calls and unchanged subprocess snippets until the first blocker",
            "fixture": "CPython SysModuleTest.tearDown, script_helper launcher options, environment and pipes",
            "assertions": "original CPython method checks the observed exception, exit status and output",
            "method_sha256": hashlib.sha256(method_bytes(source, "SysModuleTest", "test_exit")).hexdigest(),
            "opt_level": toolchain.level, "native_operations_completed": completed, "attempts": events,
        },
    }
