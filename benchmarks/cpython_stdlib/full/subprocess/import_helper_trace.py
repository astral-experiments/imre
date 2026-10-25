"""Observe calls originating in the pinned subprocess and import_helper modules."""

import _imp
import argparse
from collections import Counter
import hashlib
import importlib
import json
import os
from pathlib import Path
import sys
from types import CodeType
import unittest

HERE = Path(__file__).resolve().parent
HELPER = "test.support.import_helper"
SUITE = "test.test_subprocess"
SELECT_ID = SUITE + ".POSIXProcessTestCase.test_select_unbuffered"


def blob(path):
    data = Path(path).read_bytes()
    return hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def descendants(code):
    yield code
    for child in code.co_consts:
        if isinstance(child, CodeType):
            yield from descendants(child)


class Trace:
    def __init__(self):
        self.mon = sys.monitoring
        self.tool = next(i for i in range(6) if self.mon.get_tool(i) is None)
        self.phase = "import"
        self.pid = os.getpid()
        self.codes = []
        self.bound = []
        self.helpers = []
        self.imports = []
        self.preexisting = {}
        self.delegated = []
        self.imp = []
        self.select = []

    def start(self, code, offset):
        frame = sys._getframe(1)
        module = frame.f_globals.get("__name__")
        if module not in (SUITE, HELPER) or os.getpid() != self.pid:
            return
        if code.co_name == "<module>":
            for child in descendants(code):
                if module == HELPER or child.co_name == "test_select_unbuffered":
                    self.codes.append(child)
                    events = self.mon.events
                    self.mon.set_local_events(
                        self.tool,
                        child,
                        events.PY_START | events.PY_RETURN | events.CALL,
                    )
            return
        if module == HELPER and code.co_flags & 1:
            caller = frame.f_back
            self.helpers.append(
                dict(
                    phase=self.phase,
                    function=code.co_qualname,
                    caller=caller.f_globals.get("__name__"),
                    line=caller.f_lineno,
                )
            )
            if code.co_name == "import_module":
                loc = frame.f_locals
                self.preexisting[id(frame)] = sys.modules.get(loc["name"])
                self.imports.append(
                    dict(
                        phase=self.phase,
                        name=loc["name"],
                        deprecated=loc["deprecated"],
                        required_on=list(loc["required_on"]),
                        previously_cached=loc["name"] in sys.modules,
                    )
                )

    def call(self, code, offset, function, first):
        if os.getpid() != self.pid:
            return
        frame = sys._getframe(1)
        if (
            frame.f_globals.get("__name__") == HELPER
            and function is importlib.import_module
        ):
            self.delegated.append(
                dict(phase=self.phase, line=frame.f_lineno, name=first)
            )
        if getattr(function, "__self__", None) is _imp:
            self.imp.append(
                dict(
                    phase=self.phase,
                    caller=code.co_qualname,
                    line=frame.f_lineno,
                    builtin=function.__name__,
                    argument=first,
                )
            )
        if (
            code.co_name == "test_select_unbuffered"
            and function is sys.modules["select"].select
        ):
            assert self.phase == SELECT_ID and len(first) == 1
            self.select.append(
                dict(
                    line=frame.f_lineno,
                    first_input_is_child_stdout=first[0] is frame.f_locals["p"].stdout,
                )
            )

    def returned(self, code, offset, result):
        if os.getpid() != self.pid:
            return
        frame = sys._getframe(1)
        if frame.f_globals.get("__name__") != HELPER:
            return
        if code.co_name == "<module>":
            self.bound.append(frame.f_globals["_imp"] is _imp)
        if code.co_name == "import_module":
            assert self.imports[-1]["name"] == frame.f_locals["name"]
            self.imports[-1]["returned_cached_module"] = (
                result is sys.modules[frame.f_locals["name"]]
            )
            self.imports[-1]["returned_same_preexisting_module"] = (
                result is self.preexisting.pop(id(frame))
            )

    def __enter__(self):
        m, e = self.mon, self.mon.events
        m.use_tool_id(self.tool, "subprocess import helper")
        for event, callback in (
            (e.PY_START, self.start),
            (e.PY_RETURN, self.returned),
            (e.CALL, self.call),
        ):
            m.register_callback(self.tool, event, callback)
        # Only the cold imports need a global event, to install local monitors
        # as the original module code first runs.
        m.set_events(self.tool, e.PY_START)
        return self

    def local_only(self):
        self.mon.set_events(self.tool, 0)

    def __exit__(self, *exc):
        self.mon.set_events(self.tool, 0)
        for code in self.codes:
            self.mon.set_local_events(self.tool, code, 0)
        self.mon.free_tool_id(self.tool)


class Results(unittest.TestResult):
    def __init__(self, trace):
        super().__init__()
        self.trace, self.ids = trace, Counter()

    def startTest(self, test):
        self.trace.phase = test.id()
        self.ids[test.id()] += 1
        super().startTest(test)

    def stopTest(self, test):
        super().stopTest(test)
        self.trace.phase = "between tests"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-source", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument(
        "--cpu",
        action="store_true",
        help="also enable the two original CPU-gated methods",
    )
    args = parser.parse_args()
    assert sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 5)
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source_blobs = {
        **manifest["files"],
        "Lib/test/support/import_helper.py": "4c7eac0b7eb674f508fe40e8ad1e0f2b184fa533",
        "Lib/importlib/__init__.py": "a7d57561ead046c481d4d78bf33a6726185e399d",
        "Python/import.c": "5914722c8f75177e111dd9f3dd69fead54d0db19",
    }
    for name, expected in source_blobs.items():
        assert blob(args.cpython_source / name) == expected, name
    assert HELPER not in sys.modules and SUITE not in sys.modules
    with Trace() as trace:
        trace.phase = "runner setup"
        support = importlib.import_module("test.support")
        # regrtest's default resource configuration, before decorators run.
        support.use_resources = {"cpu": True} if args.cpu else {}
        trace.phase = "import"
        upstream = importlib.import_module(SUITE)
        trace.local_only()
        helper = sys.modules[HELPER]
        assert blob(upstream.__file__) == source_blobs["Lib/test/test_subprocess.py"]
        assert (
            blob(helper.__file__) == source_blobs["Lib/test/support/import_helper.py"]
        )
        assert blob(importlib.__file__) == source_blobs["Lib/importlib/__init__.py"]
        assert trace.bound == [True]
        trace.phase = "discovery"
        suite = unittest.defaultTestLoader.loadTestsFromModule(upstream)
        assert not unittest.defaultTestLoader.errors
        trace.phase = "fixtures"
        result = Results(trace)
        suite.run(result)
        observed = list(trace.imp)
        trace.phase = "positive controls"
        with helper.frozen_modules(False):
            pass
        try:
            with helper.multi_interp_extensions_check(True):
                raise AssertionError("unexpected main-interpreter extension override")
        except RuntimeError:
            pass
    ref = json.loads((HERE / "reference-linux.json").read_text())
    assert result.wasSuccessful(), (result.errors, result.failures)
    assert (
        result.ids == Counter(ref["reference"].keys())
        and result.testsRun == ref["denominator"]
    )
    skips = {case.id(): reason for case, reason in result.skipped}
    expected_skips = {
        name: val["skipped"] for name, val in ref["reference"].items() if val != "pass"
    }
    if args.cpu:
        gated = {
            name
            for name, reason in expected_skips.items()
            if reason == "resource 'cpu' is not enabled"
        }
        assert gated == {
            f"{SUITE}.{case}.test_leaking_fds_on_error"
            for case in ("ProcessTestCase", "ProcessTestCaseNoPoll")
        }
        expected_skips = {k: v for k, v in expected_skips.items() if k not in gated}
    assert skips == expected_skips, {
        k: (expected_skips.get(k), skips.get(k))
        for k in expected_skips.keys() | skips.keys()
        if expected_skips.get(k) != skips.get(k)
    }
    assert observed == []
    assert trace.imports == [
        dict(
            phase=SELECT_ID,
            name="select",
            deprecated=False,
            required_on=[],
            previously_cached=True,
            returned_cached_module=True,
            returned_same_preexisting_module=True,
        )
    ]
    assert trace.delegated == [dict(phase=SELECT_ID, line=80, name="select")]
    assert trace.select == [dict(line=3316, first_input_is_child_stdout=True)]
    assert [(r["builtin"], r["argument"]) for r in trace.imp] == [
        ("_override_frozen_modules_for_tests", -1),
        ("_override_frozen_modules_for_tests", 0),
        ("_override_multi_interp_extensions_check", 1),
    ]
    data = dict(
        schema="imre.subprocess.import-helper/v1",
        cpython_commit=manifest["commit"],
        execution="reference-only",
        blobs=source_blobs,
        denominator=result.testsRun,
        resources="cpu" if args.cpu else "default",
        passed=result.testsRun - len(skips),
        skipped=len(skips),
        native_credits=0,
        original_import_bound_real_imp=trace.bound[0],
        helper_entries=[v for v in trace.helpers if v["phase"] != "positive controls"],
        import_module_calls=trace.imports,
        delegated_importlib_calls=trace.delegated,
        select_calls=trace.select,
        helper_issued_imp_calls=observed,
        separate_positive_control=trace.imp,
    )
    args.result.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
