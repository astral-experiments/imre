"""Transport native sys/builtins operations into the original CPython fixture."""
import ast
from contextlib import ExitStack
from pathlib import Path
import selectors
import subprocess
import sys
import unittest
from unittest.mock import patch


def run_display_case(test_sys, executable, work):
    # The child writes through its real stdout binding. Feed only the newly written bytes
    # into the upstream capture context; replies to namespace operations travel via stderr.
    path = Path(work) / "display.stdout"
    calls = []
    with ExitStack() as stack:
        writer = stack.enter_context(path.open("wb"))
        reader = stack.enter_context(path.open("rb"))
        child = stack.enter_context(subprocess.Popen(
            [executable], stdin=subprocess.PIPE, stdout=writer, stderr=subprocess.PIPE,
        ))
        stack.callback(lambda: child.kill() if child.poll() is None else None)
        ready = stack.enter_context(selectors.DefaultSelector())
        ready.register(child.stderr, selectors.EVENT_READ)

        def invoke(name, *args):
            calls.append(name)
            child.stdin.write(("\n".join([name, *map(str, args)]) + "\n").encode())
            child.stdin.flush()
            if not ready.select(10):
                raise TimeoutError(f"no native response for {name}")
            response = child.stderr.readline().decode().rstrip("\n")
            captured = reader.read()
            if captured:
                sys.stdout.write(captured.decode("utf-8"))
            if response == "ok":
                return None
            kind, _, detail = response.partition(" ")
            if kind == "value":
                return ast.literal_eval(detail)
            errors = {"TypeError": TypeError, "RuntimeError": RuntimeError,
                      "AttributeError": AttributeError}
            if kind in errors:
                raise errors[kind](detail)
            raise AssertionError((name, child.poll(), response))

        class NativeBuiltins:
            @property
            def _(self):
                return invoke("get-last")

            @_.deleter
            def _(self):
                invoke("delete-last")

        class NativeSys:
            def __displayhook__(self, *args):
                if not args:
                    return invoke("display-no-args")
                if args == (None,):
                    return invoke("display-none")
                if len(args) == 1 and type(args[0]) is int:
                    return invoke("display-int", args[0])
                raise AssertionError(("unexpected display operand", args))

            @property
            def stdout(self):
                invoke("save-stdout")
                return saved_handle

            @stdout.deleter
            def stdout(self):
                invoke("delete-stdout")

            @stdout.setter
            def stdout(self, handle):
                if handle is not saved_handle:
                    raise AssertionError("unknown stdout handle")
                invoke("restore-stdout")

        saved_handle = object()
        case = test_sys.DisplayHookTest("test_original_displayhook")
        with patch.object(test_sys, "sys", NativeSys()), patch.object(test_sys, "builtins", NativeBuiltins()):
            result = unittest.TestResult()
            case.run(result)
        if result.errors or result.failures or result.skipped:
            raise AssertionError((case.id(), result.errors, result.failures, result.skipped))
        child.stdin.write(b"stop\n")
        child.stdin.flush()
        if child.wait(10) or child.stderr.read() or reader.read():
            raise AssertionError("unexpected native stream output on shutdown")
        return {"execution": "host-driven-native-operation", "status": "pass", "calls": calls,
                "fixture": "original CPython unittest and support.captured_stdout contexts",
                "operation": "one native process retains the displayed object, deletes builtins._, "
                             "and deletes and restores its actual sys.stdout binding"}
