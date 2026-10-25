"""Observe the release's real _imp calls without replacing either builtin or fixture."""

import _imp
import _interpreters
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from test.support import import_helper


COMMIT = "5607950ef232dad16d75c0cf53101d9649d89115"
HELPER = "4c7eac0b7eb674f508fe40e8ad1e0f2b184fa533"
PACKAGE = "74b64ed86a3183db5a54166d89cf2fb227281e06"
IMPORT_C = "5914722c8f75177e111dd9f3dd69fead54d0db19"
FROZEN = _imp._override_frozen_modules_for_tests
EXTENSIONS = _imp._override_multi_interp_extensions_check


def blob(path):
    raw = Path(path).read_bytes()
    return hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest()


def modules():
    return {k: v for k, v in sys.modules.items()
            if k in ("json", "_json") or k.startswith(("json.", "_json."))}


def accelerator():
    return ("absent" if "_json" not in sys.modules else
            "blocked" if sys.modules["_json"] is None else "present")


class Trace:
    """Local CPython monitoring events include the actual C callable and first argument."""

    def __init__(self):
        self.calls = []
        self.fixtures = []
        self.active = None
        self.mon = sys.monitoring
        self.tool = next(i for i in range(6) if self.mon.get_tool(i) is None)

    def entering(self, code, offset):
        frame = sys._getframe(1)
        if frame.f_locals["name"] != "json":
            return
        caller = frame.f_back
        assert caller.f_globals["__name__"] == "test.test_json" and self.active is None
        loc = frame.f_locals
        row = {"package_line": caller.f_lineno, "fresh": list(loc["fresh"]),
               "blocked": list(loc["blocked"]), "usefrozen": loc["usefrozen"],
               "deprecated": loc["deprecated"], "calls": []}
        self.active = row, modules()
        self.fixtures.append(row)

    def leaving(self, code, offset, result):
        if self.active is None or sys._getframe(1).f_locals["name"] != "json":
            return
        row, before = self.active
        after = modules()
        assert before.keys() == after.keys() and all(before[k] is after[k] for k in before)
        row["restored_module_objects"] = sorted(before)
        row["scanner_module"] = result.scanner.make_scanner.__module__
        self.active = None

    def event(self, outcome):
        def observed(code, offset, target, arg):
            if target is not FROZEN and target is not EXTENSIONS:
                return
            line = next(line for lo, hi, line in code.co_lines() if lo <= offset < hi)
            if outcome == "call":
                row = {"builtin": target.__name__, "line": line, "arg": arg,
                       "_json": accelerator()}
                self.calls.append(row)
                if self.active is not None:
                    self.active[0]["calls"].append(len(self.calls) - 1)
            else:
                row = self.calls[-1]
                assert (row["builtin"], row["line"], row["arg"]) == (target.__name__, line, arg)
                row["outcome"] = outcome
        return observed

    def extension_line(self, code, line):
        if line == 125:  # The original generator now holds the C return value in old.
            row = self.calls[-1]
            assert (row["line"], row["outcome"]) == (123, "return")
            row["returned_previous"] = sys._getframe(1).f_locals["old"]

    def __enter__(self):
        mon, ev = self.mon, self.mon.events
        mon.use_tool_id(self.tool, "original JSON import fixture trace")
        self.callbacks = {ev.PY_START: self.entering, ev.PY_RETURN: self.leaving, ev.LINE: self.extension_line,
                          ev.CALL: self.event("call"), ev.C_RETURN: self.event("return"),
                          ev.C_RAISE: self.event("raise")}
        for event, callback in self.callbacks.items():
            mon.register_callback(self.tool, event, callback)
        self.sources = {import_helper.import_fresh_module.__code__: ev.PY_START | ev.PY_RETURN,
                        import_helper.frozen_modules.__wrapped__.__code__: ev.CALL,
                        import_helper.multi_interp_extensions_check.__wrapped__.__code__: ev.CALL | ev.LINE}
        for code, events in self.sources.items():
            mon.set_local_events(self.tool, code, events)
        return self

    def __exit__(self, *exc):
        for code in self.sources:
            self.mon.set_local_events(self.tool, code, 0)
        for event in self.callbacks:
            self.mon.register_callback(self.tool, event, None)
        self.mon.free_tool_id(self.tool)


def signature(trace):
    return [(r["line"], r["arg"], r["outcome"]) for r in trace.calls]


def subinterpreter_control():
    assert blob(import_helper.__file__) == HELPER
    with Trace() as trace:
        with import_helper.multi_interp_extensions_check(False):
            try:
                with import_helper.multi_interp_extensions_check(True):
                    raise LookupError("exercise the original finally")
            except LookupError:
                pass
    assert signature(trace) == [(123, -1, "return"), (123, 1, "return"),
                                (127, -1, "return"), (127, 0, "return")]
    assert [r["returned_previous"] for r in trace.calls if r["line"] == 123] == [0, -1]
    return trace.calls


def controls():
    baseline = _imp.is_frozen("__hello__")
    with Trace() as trace:
        with import_helper.frozen_modules(False):
            off = _imp.is_frozen("__hello__")
        with import_helper.frozen_modules(True):
            on = _imp.is_frozen("__hello__")
        try:
            with import_helper.multi_interp_extensions_check(True):
                raise AssertionError("main interpreter entered the extension override")
        except RuntimeError as exc:
            message = str(exc)
    assert (off, on, _imp.is_frozen("__hello__")) == (False, True, baseline)
    assert signature(trace) == [(103, -1, "return"), (107, 0, "return"),
                                (103, 1, "return"), (107, 0, "return"), (123, 1, "raise")]
    assert message == "_imp._override_multi_interp_extensions_check() cannot be used in the main interpreter"
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "sub.json"
        script = f"""import sys, json
from pathlib import Path
sys.path[:0] = {[str(Path(__file__).resolve().parent), str(Path(import_helper.__file__).resolve().parents[2])]!r}
from import_helper_trace import subinterpreter_control
Path({str(path)!r}).write_text(json.dumps(subinterpreter_control()))
"""
        interp = _interpreters.create()
        try:
            failure = _interpreters.exec(interp, script)
            assert failure is None, failure
            nested = json.loads(path.read_bytes())
        finally:
            _interpreters.destroy(interp)
    return {"main": trace.calls, "main_extension_error": message,
            "is_frozen___hello__": {"baseline": baseline, "off": off, "on": on,
                                    "after": _imp.is_frozen("__hello__")},
            "subinterpreter_nested_extension_override": nested}


class Results(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.ids = Counter()

    def startTest(self, test):
        self.ids[test.id()] += 1
        super().startTest(test)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path, default=Path(import_helper.__file__).resolve().parents[3])
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5) and sys.implementation.name == "cpython"
    assert blob(import_helper.__file__) == HELPER and "test.test_json" not in sys.modules
    assert blob(args.cpython_source / "Python/import.c") == IMPORT_C
    with Trace() as trace:
        from test import test_json
        assert blob(test_json.__file__) == PACKAGE
        result = Results()
        unittest.defaultTestLoader.loadTestsFromModule(test_json).run(result)
    assert result.wasSuccessful() and result.testsRun == 216 and len(result.ids) == 212, (
        result.errors, result.failures, result.testsRun)
    assert len(result.skipped) == 1 and result.skipped[0][0].id().endswith("test_overflow")
    assert signature(trace) == [(103, -1, "return"), (107, 0, "return")] * 2
    assert all(row["builtin"] == FROZEN.__name__ for row in trace.calls)
    assert [(r["package_line"], r["fresh"], r["blocked"], r["scanner_module"])
            for r in trace.fixtures] == [(11, ["_json"], [], "_json"),
                                         (12, [], ["_json"], "json.scanner")]
    assert [r["_json"] for r in trace.calls] == ["absent", "present", "blocked", "blocked"]
    report = {"cpython_commit": COMMIT, "platform": sys.platform, "gil_enabled": sys._is_gil_enabled(),
              "execution": "stock CPython reference only; monitored calls in the suite interpreter",
              "blobs": {"Lib/test/support/import_helper.py": HELPER,
                        "Lib/test/test_json/__init__.py": PACKAGE, "Python/import.c": IMPORT_C},
              "suite": {"occurrences": result.testsRun, "distinct_ids": len(result.ids),
                        "duplicate_ids": {k: v for k, v in result.ids.items() if v > 1},
                        "passed": result.testsRun-len(result.skipped),
                        "skipped": {t.id(): reason for t, reason in result.skipped}},
              "fixtures": trace.fixtures, "original_calls": trace.calls,
              "original_extension_override_calls": 0, "separate_reference_controls": controls()}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
