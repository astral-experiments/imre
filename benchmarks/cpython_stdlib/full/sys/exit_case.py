"""Drive the unchanged test_exit through native calls and exact child sources."""
import ast
import hashlib
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_native import NativeCompileError, method_bytes, retain_partial_observation


CASE_ID = "test.test_sys.SysModuleTest.test_exit"


def observer(module, toolchain, directory, source):
    method_sha256 = hashlib.sha256(method_bytes(source, "SysModuleTest", "test_exit")).hexdigest()

    def execute(case, original):
        return observe(module, toolchain, directory, method_sha256, original)

    return execute


def observe(module, toolchain, directory, method_sha256, original):
    from test.support import script_helper

    # script_helper probes the CPython fixture executable once to choose its
    # bootstrap flags. That probe is not an execution of the module operation.
    script_helper.interpreter_requires_environment()
    events = []

    def build(program, kind, **receipt):
        path = directory / str(len(events))
        path.mkdir()
        data = program.encode()
        event = {"kind": kind, "source_sha256": hashlib.sha256(data).hexdigest(), **receipt}
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
        binary, event = build(program, "caught sys.exit", call=call)
        run = subprocess.run([binary], capture_output=True, timeout=15)
        event.update(returncode=run.returncode, stdout_hex=run.stdout.hex(), stderr_hex=run.stderr.hex())
        run.check_returncode()
        if run.stderr:
            raise AssertionError(run.stderr)
        name, literal = run.stdout.decode().splitlines()
        event.update(exception=name, payload=literal)
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
        binary, event = build(program, "original child", source=program, launcher_flags=argv[1:-2],
                              PYTHONIOENCODING=kwargs["env"].get("PYTHONIOENCODING"))

        class Child(subprocess.Popen):
            def communicate(self, *args, **kwargs):
                stdout, stderr = super().communicate(*args, **kwargs)
                event.update(returncode=self.returncode, stdout_hex=stdout.hex(), stderr_hex=stderr.hex())
                return stdout, stderr

        return Child([binary], **kwargs)

    def observation():
        completed = sum("exception" in e or e["kind"] == "original child" and "returncode" in e for e in events)
        return {
            "roles": roles(body="cpython", assertions="cpython",
                           operation="imre" if any("returncode" in e for e in events) else "unobserved", fixture="cpython"),
            "evidence": {
                "operation": "native sys.exit calls and unchanged subprocess snippets",
                "fixture": "CPython SysModuleTest.tearDown, script_helper launcher options, environment and pipes",
                "assertions": "original CPython method checks the observed exception, exit status and output",
                "method_sha256": method_sha256, "opt_level": toolchain.level,
                "native_operations_completed": completed, "attempts": events,
            },
        }

    child_fixture = SimpleNamespace(PIPE=subprocess.PIPE, Popen=popen, _cleanup=subprocess._cleanup)
    with (retain_partial_observation(observation),
          patch.object(module, "sys", SimpleNamespace(exit=exit_call)),
          patch.object(script_helper, "subprocess", child_fixture)):
        original()
        if (sum("exception" in e for e in events) != 6
                or sum(e["kind"] == "original child" and "returncode" in e for e in events) != 7):
            raise AssertionError("incomplete original test_exit operation sequence")
    return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
            "fixture_execution": "CPython module/class/instance hooks", "observation": observation()}
