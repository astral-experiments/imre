"""Bridge the original cache assertions to one native logging process."""
import ast
from collections import Counter
from pathlib import Path
import re
import selectors
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles

HERE = Path(__file__).resolve().parent


def check_cache_values(toolchain, directory):
    def output(command):
        result = subprocess.run(command, capture_output=True, timeout=30)
        return result.returncode, result.stdout, result.stderr

    for source in (HERE / "cache_values.py", HERE / "standalone_values.py"):
        native = toolchain.build(source.read_bytes(), directory)
        expected = output([sys.executable, "-I", "-B", source])
        actual = output([native])
        if expected[0] or actual != expected:
            raise AssertionError(f"{source.name}: native {actual!r}; CPython {expected!r}")


def cache_assertion_blocker(exc, directory):
    """Accept only the unsupported shared assertion at this exact upstream statement."""
    match = re.search(r"case\.py:(\d+):\d+: Imre: (expected (?:bool \| )?int, got dict\[int, bool\]; annotations are enforced contracts)", str(exc))
    if match:
        statement = (directory / "case.py").read_text().splitlines()[int(match[1]) - 1].strip()
        if statement == "self.assertEqual(logger2._cache, {})":
            return {"source": "Lib/test/test_logging.py:6220", "statement": statement,
                    "diagnostic": match[2]}
    return None


def host_cache_case(toolchain, directory, blocker):
    binary = toolchain.build((HERE / "cache_driver.py").read_bytes(), directory)

    def drive(case, method):
        original = sys.modules[type(case).__module__]
        upstream = original.logging
        ops = []
        with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, encoding="utf-8") as child, \
             selectors.DefaultSelector() as ready:
            ready.register(child.stdout, selectors.EVENT_READ)

            def read():
                if not ready.select(timeout=15):
                    child.kill()
                    raise AssertionError("native logger did not answer")
                reply = child.stdout.readline()
                if not reply:
                    raise AssertionError("native logger exited before replying")
                return reply.removesuffix("\n")

            def request(op, *args):
                ops.append(op)
                for arg in (op, *args):
                    value = str(arg)
                    if "\n" in value or "\r" in value:
                        raise ValueError("cache argument contains a newline")
                    child.stdin.write(value + "\n")
                child.stdin.flush()
                return read()

            class Logger:
                def __init__(self, name):
                    self.name = name

                def setLevel(self, level):
                    assert request("set", self.name, level) == "ok"

                def getEffectiveLevel(self):
                    return int(request("effective", self.name))

                def isEnabledFor(self, level):
                    return ast.literal_eval(request("enabled", self.name, level))

                @property
                def _cache(self):
                    return ast.literal_eval(request("cache", self.name))

            def disable():
                assert request("disable") == "ok"

            api = SimpleNamespace(getLogger=lambda name: Logger(request("get", name)),
                                  disable=disable, DEBUG=upstream.DEBUG, ERROR=upstream.ERROR,
                                  NOTSET=upstream.NOTSET, CRITICAL=upstream.CRITICAL)
            try:
                level, present = read().split(":")
                assert (int(level), ast.literal_eval(present)) == (case.root_logger.level, case.root_logger.hasHandlers())
                assert case.root_hdlr in case.root_logger.handlers
                with patch.object(original, "logging", api), patch.object(case, "root_logger", Logger(case.root_logger.name)):
                    method()
            finally:
                if child.poll() is None:
                    child.stdin.write("stop\n")
                child.stdin.close()
                if child.wait(timeout=15) or child.stderr.read():
                    raise AssertionError("native logger cache driver failed")
        counts = Counter(ops)
        assert counts == {"get": 2, "set": 3, "effective": 4, "enabled": 13, "cache": 12, "disable": 1}, counts
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "Persistent native loggers, live caches, level updates and global suppression",
                             "fixture": "Original LoggerTest/BaseTest hooks and 29 CPython assertions",
                             "opt_level": toolchain.level, "native_calls": dict(counts),
                             "native_body_blocker": blocker}}
    return drive
