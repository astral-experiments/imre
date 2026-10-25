"""Replay the original fixture's calls against native TextWrapper instances."""

import ast
from collections import Counter
from contextlib import ExitStack
import hashlib
import subprocess
import textwrap
import unittest
from unittest.mock import patch

from suite_evidence import roles


def wrapper_cases(module, tools, directory, denominator):
    # Keep the same object for an entire test, including writes between calls.
    # CPython owns the fixture; native output must both match the captured
    # observation and satisfy the original assertion on its second execution.
    events = []
    expected = []
    methods = Counter()
    source = ["from textwrap import TextWrapper, wrap, fill, shorten", "", "def main() -> None:"]
    replies = None

    class Result(unittest.TestResult):
        current = None

        def startTest(self, case):
            super().startTest(case)
            self.current = case.id()

        def stopTest(self, case):
            self.current = None
            super().stopTest(case)

    def event(command, *, result=False):
        nonlocal cursor
        item = (run.current, command, result)
        if replies is None:
            events.append(item)
            if result:
                source.extend(
                    [
                        "    try:",
                        "        print('O' + repr(" + command + "))",
                        "    except ValueError as exc:",
                        "        print('V' + str(exc))",
                        "    except TypeError as exc:",
                        "        print('T' + str(exc))",
                    ]
                )
            else:
                source.append("    " + command)
        elif item != events[cursor]:
            raise AssertionError(("changed upstream call order", cursor, item, events[cursor]))
        cursor += 1

    def call(name, original, args, kwargs):
        nonlocal dispatched
        command = name + "(" + arguments(args, kwargs) + ")"
        event(command, result=True)
        if replies is None:
            methods[run.current] += 1
            try:
                tag, value = "O", original(*args, **kwargs)
            except (TypeError, ValueError) as exc:
                tag, value = ("T" if isinstance(exc, TypeError) else "V"), str(exc)
            expected.append((tag, value))
        else:
            tag, value = replies[dispatched]
        dispatched += 1
        if tag == "T":
            raise TypeError(value)
        if tag == "V":
            raise ValueError(value)
        return value

    def arguments(args, kwargs):
        return ", ".join([*map(repr, args), *(f"{k}={v!r}" for k, v in kwargs.items())])

    class Proxy:
        def __init__(self, *args, **kwargs):
            nonlocal serial
            self._name = f"w{serial}"
            serial += 1
            self._reference = textwrap.TextWrapper(*args, **kwargs)
            event(f"{self._name} = TextWrapper({arguments(args, kwargs)})")

        def __setattr__(self, name, value):
            if name.startswith("_"):
                object.__setattr__(self, name, value)
            else:
                event(f"{self._name}.{name} = {value!r}")
                setattr(self._reference, name, value)

        def __getattr__(self, name):
            original = getattr(self._reference, name)
            if not callable(original):
                return original
            return lambda *a, **kw: call(self._name + "." + name, original, a, kw)

    def public(name):
        fn = getattr(textwrap, name)
        return lambda *a, **kw: call(name, fn, a, kw)

    def execute():
        nonlocal run, serial, cursor, dispatched
        run = Result()
        serial = cursor = dispatched = 0
        with ExitStack() as stack:
            stack.enter_context(patch.object(module, "TextWrapper", Proxy))
            for name in ("wrap", "fill", "shorten"):
                stack.enter_context(patch.object(module, name, public(name)))
            unittest.defaultTestLoader.loadTestsFromModule(module).run(run)
        if run.testsRun != denominator or run.skipped or not run.wasSuccessful():
            raise AssertionError((run.testsRun, run.skipped, run.errors, run.failures))

    run = serial = cursor = dispatched = None
    execute()
    fixture = ("\n".join([*source, "", "main()", ""])).encode()
    binary = tools.build(fixture, directory)
    result = subprocess.run([binary], capture_output=True, check=True, timeout=90)
    lines = result.stdout.split(b"\n")
    if result.stderr or lines.pop() != b"" or len(lines) != len(expected):
        raise AssertionError((result.stderr, len(lines), len(expected)))
    replies = []
    for output, (want_tag, want) in zip(lines, expected, strict=True):
        tag, rest = output[:1].decode(), output[1:].decode()
        value = ast.literal_eval(rest) if tag == "O" else rest
        if tag != want_tag or type(value) is not type(want) or value != want:
            raise AssertionError((tag, value, want_tag, want))
        replies.append((tag, value))
    execute()
    if (cursor, dispatched) != (len(events), len(expected)):
        raise AssertionError(("incomplete replay", cursor, dispatched))
    return {
        (name, 1): {
            "outcome": "pass",
            "roles": roles(
                body="cpython", assertions="cpython", operation="imre", fixture="cpython"
            ),
            "evidence": {
                "operation": "textwrap.TextWrapper, wrap, fill, or shorten",
                "fixture": "Unmodified host fixture; native wrapper retains construction and mutations across calls",
                "native_calls": count,
                "opt_level": tools.level,
                "input_sha256": hashlib.sha256(fixture).hexdigest(),
            },
        }
        for name, count in methods.items()
    }
