"""Run original BasicTest bodies with CPython fixtures and native status operations."""
from collections import Counter
from contextlib import ExitStack
from http import HTTPStatus
import select
import subprocess
import unittest
from unittest.mock import patch

from verify import HERE

CATEGORIES = ("is_informational", "is_success", "is_redirection", "is_client_error", "is_server_error")


def hosted(tool, work, case):
    binary = tool.build((HERE / "status_worker.py").read_bytes(), work)
    calls = Counter()
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True) as worker:
        def exchange(member, operation, bound=0):
            worker.stdin.write(f"{member.value}\n{operation}\n{bound}\n")
            worker.stdin.flush()
            if not select.select([worker.stdout], [], [], 5)[0]:
                raise TimeoutError("native status worker did not reply")
            value = worker.stdout.readline()
            if value not in {"True\n", "False\n"}:
                raise ValueError(f"unexpected native status reply: {value!r}")
            calls[operation] += 1
            return value == "True\n"

        try:
            # CPython owns the TestCase, enum registry, attribute dispatch and assertions.
            # Each observed comparison or category read crosses the pipe to a native member.
            with ExitStack() as patches:
                for name in CATEGORIES:
                    patches.enter_context(patch.object(HTTPStatus, name,
                        property(lambda member, name=name: exchange(member, name))))
                patches.enter_context(patch.object(HTTPStatus, "__ge__", lambda member, n: exchange(member, "ge", n)))
                patches.enter_context(patch.object(HTTPStatus, "__le__", lambda member, n: exchange(member, "le", n)))
                result = unittest.TestResult()
                unittest.TestSuite([case]).run(result)
                if result.testsRun != 1 or result.skipped or not result.wasSuccessful():
                    raise AssertionError((result.skipped, result.failures, result.errors))
            worker.stdin.write("quit\n")
            worker.stdin.flush()
            remaining, stderr = worker.communicate(timeout=5)
            if worker.returncode or remaining or stderr:
                raise RuntimeError((worker.returncode, remaining, stderr))
        finally:
            if worker.poll() is None:
                worker.kill()
    count = len(HTTPStatus.__members__)
    if case._testMethodName == "test_httpstatus_range":
        if calls != {"ge": count, "le": count}:
            raise AssertionError(calls)
    elif not all(calls[name] == count for name in CATEGORIES):
        raise AssertionError(calls)
    return dict(sorted(calls.items()))
