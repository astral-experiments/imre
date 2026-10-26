"""Run the original LoggerTest enablement methods and verify their registered restorations."""
import argparse
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import re
import selectors
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
from native_standalone import SOURCE, standalone_fixture
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures

CASES = {
    "test_is_enabled_for": (6172, "self.addCleanup(setattr, self.logger.manager, 'disable', old_disable)"),
    "test_is_enabled_for_disabled_logger": (6182, "self.addCleanup(setattr, self.logger, 'disabled', old_disabled)"),
}
ADAPTER_CASE = (5850, "self.addCleanup(setattr, self.adapter.logger.manager, 'disable',")


class WithCleanups:
    def __init__(self, tools, initial, logger="case.logger"):
        self.tools = tools
        self.level = tools.level
        self.initial = initial
        self.logger = logger

    def build(self, source, work):
        limit, disabled = self.initial
        # Append to the shared runner's entry point; the upstream method is unchanged.
        return self.tools.build(source + (
            f"assert case.doCleanups()\nassert {self.logger}.manager.disable == {limit!r}\n"
            f"assert {self.logger}.disabled is {disabled!r}\n").encode(), work)


def host_body(binary, case, method, name, initial, evidence, *, adapter=False):
    stack = ExitStack()
    child = stack.enter_context(subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                                stderr=subprocess.PIPE, text=True, encoding="utf-8"))
    ready = stack.enter_context(selectors.DefaultSelector())
    ready.register(child.stdout, selectors.EVENT_READ)
    calls = []

    def ask(op, *values):
        calls.append([op, *values])
        child.stdin.write("\n".join(str(v) for v in (op, *values)) + "\n")
        child.stdin.flush()
        if not ready.select(timeout=15):
            child.kill()
            raise AssertionError("native logger did not reply to " + op)
        reply = child.stdout.readline()
        if not reply:
            raise AssertionError("native logger exited during " + op)
        return reply.rstrip("\n")

    def boolean(reply):
        if reply not in ("True", "False"):
            raise AssertionError(reply)
        return reply == "True"

    class Manager:
        @property
        def disable(self):
            return int(ask("limit"))

        @disable.setter
        def disable(self, value):
            assert type(value) is int and ask("set-limit", value) == "ok"

    manager = Manager()

    class Logger:
        @property
        def manager(self):
            assert boolean(ask("manager"))
            return manager

        @property
        def disabled(self):
            return boolean(ask("disabled"))

        @disabled.setter
        def disabled(self, value):
            assert type(value) is bool and ask("set-disabled", value) == "ok"

        def isEnabledFor(self, level):
            return boolean(ask("enabled", level))

    def finish():
        try:
            if "body_calls" in evidence:
                cleanup = calls[evidence.pop("_body_end"):]
                expected = [["set-limit", initial[0]]]
                if name.endswith("disabled_logger"):
                    expected.append(["set-disabled", initial[1]])
                assert cleanup == expected, cleanup
                restored = (int(ask("limit")), boolean(ask("disabled")))
                assert restored == initial, restored
                evidence.update(cleanup_calls=cleanup, restored=dict(disable=restored[0], disabled=restored[1]))
        finally:
            try:
                if child.poll() is None:
                    child.stdin.write("stop\n")
                    child.stdin.flush()
                status = child.wait(timeout=15)
                error = child.stderr.read()
                if status or error:
                    raise AssertionError((status, error))
            finally:
                if child.poll() is None:
                    child.kill()
                stack.close()

    case.addCleanup(finish)
    assert (int(ask("limit")), boolean(ask("disabled"))) == initial
    start = len(calls)
    logger = Logger()
    receiver = SimpleNamespace(logger=logger, isEnabledFor=logger.isEnabledFor) if adapter else logger
    with patch.object(case, "adapter" if adapter else "logger", receiver):
        method()
    evidence["body_calls"] = calls[start:]
    evidence["_body_end"] = len(calls)
    return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
            "evidence": evidence}


def manager_cases(tools, work, *, adapter_fixture=None):
    adapter = adapter_fixture is not None
    owner = "LoggerAdapterTest" if adapter else "LoggerTest"
    selected = {"test_is_enabled_for": ADAPTER_CASE} if adapter else CASES
    prefix = f"test.test_logging.{owner}."
    subject = "case.adapter" if adapter else "case.logger"
    logger = "case.adapter.logger" if adapter else "case.logger"
    # These functions resolve the original receiver in the native process.
    access = (f"\ndef checked_logger(case: NativeCase) -> logging.Logger:\n    return {logger}\n"
              f"\ndef checked_enabled(case: NativeCase, level: int) -> bool:\n    return {subject}.isEnabledFor(level)\n")
    fixture_for = adapter_fixture or standalone_fixture

    def callback(name, directory):
        def execute(case, method):
            fixture = fixture_for(case)
            original_logger = case.adapter.logger if adapter else case.logger
            initial = (original_logger.manager.disable, original_logger.disabled)
            detail = ("LoggerAdapter and manager reads, writes, enablement and registered cleanup" if adapter else
                      "Logger and manager reads, writes, enablement and registered cleanup")
            try:
                run = native_method(WithCleanups(tools, initial, logger), directory, SOURCE, owner,
                                    prefix + name, fixture, operation=("imre", detail))
            except NativeCompileError as exc:
                line, expected = selected[name]
                observed = re.search(r"case\.py:(\d+):\d+: Imre: (unknown class method)$", str(exc), re.MULTILINE)
                if (not observed or (directory / "case.py").read_text().splitlines()[int(observed[1]) - 1].strip() != expected
                        or SOURCE.read_text().splitlines()[line - 1].strip() != expected):
                    raise
                binary = tools.build((fixture + access + "\n").encode() + (HERE / "manager_driver.py").read_bytes(), directory)
                return host_body(binary, case, method, name, initial, {
                    "operation": detail, "opt_level": tools.level,
                    "method_sha256": hashlib.sha256(method_bytes(SOURCE, owner, name)).hexdigest(),
                    "fixture": ("Original CPython LoggerAdapterTest and unittest cleanup stack; native root logger, "
                                "adapter, recording handler, manager and cleanup writes" if adapter else
                                "Original CPython LoggerTest/BaseTest and unittest cleanup stack; native root stream, "
                                "detached logger, recording handler, manager and cleanup writes"),
                    "native_body_blocker": {"source": f"Lib/test/test_logging.py:{line}", "statement": expected,
                                            "diagnostic": observed[2]}}, adapter=adapter)
            result = run(case, method)
            result["observation"]["evidence"]["cleanup"] = "Native doCleanups and both restored attributes checked"
            return result
        return execute

    callbacks = {}
    for name in selected:
        directory = work / name
        directory.mkdir()
        callbacks[prefix + name] = callback(name, directory)
    return callbacks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("requires pinned CPython 3.14.5 with assertions")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"] | manifest["reference_files"])
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    from test import test_logging
    assert Path(test_logging.__file__).read_bytes() == SOURCE.read_bytes()
    report = {"cpython_commit": manifest["commit"], "denominator": 272, "levels": {}}
    for opt in (0, 3):
        suite = load_suite(manifest["module"])
        assert len(list(cases(suite))) == report["denominator"]
        with tempfile.TemporaryDirectory() as temp:
            callbacks = manager_cases(Toolchain(args.imre, args.runtime, args.clang, opt), Path(temp))
            results, failures, skipped = run_in_fixtures(suite, callbacks)
            if failures or skipped or set(results) != set(callbacks):
                raise AssertionError((failures, skipped, results))
            records = {}
            for name, (value,) in results.items():
                row = value.get("observation", value)
                if row["roles"]["body"] == "cpython":
                    assert row["evidence"]["restored"] == {"disable": 0, "disabled": False}
                    assert "_body_end" not in row["evidence"]
                records[name] = row
                print(f"-O{opt} {name}: {row['roles']}", flush=True)
            report["levels"][str(opt)] = records
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
