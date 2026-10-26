"""Observe PEP 421 attributes and version slices from the native sys namespace."""

import ast
import hashlib
from pathlib import Path
import subprocess
from unittest.mock import patch
from suite_evidence import roles
from suite_native import method_bytes


def metadata_observer(test_sys, binary, level):
    calls = []

    def read(name, *args):
        run = subprocess.run([binary, name, *args], check=True, capture_output=True, timeout=15)
        if run.stderr:
            raise AssertionError(run.stderr)
        calls.append([name, *args])
        return ast.literal_eval(run.stdout.decode())

    class Version:
        def __getitem__(self, index):
            if index != slice(None, 2):
                raise ValueError(index)
            return read("prefix")

        def __getattr__(self, field):
            if field not in {"major", "minor", "micro", "releaselevel", "serial"}:
                raise AttributeError(field)
            return read(field)

    class Implementation:
        def __getattr__(self, field):
            if not read("has", field):
                raise AttributeError(field)
            value = read(field)
            if field == "version":
                if not isinstance(value, tuple):
                    raise AssertionError(value)
                return Version()
            return value

    class NativeSys:
        implementation = Implementation()

    native_sys = NativeSys()
    def execute(case, original):
        calls.clear()
        # The original helper reads the implementation name before testing its CPython-only branch.
        with patch.object(test_sys, "sys", native_sys), patch.object(test_sys.support, "sys", native_sys):
            original()
        if not all([name] in calls for name in ("major", "minor", "micro", "releaselevel", "serial",
                                                 "prefix", "hexversion", "name", "supports_isolated_interpreters")):
            raise AssertionError((case.id(), calls))
        return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                "fixture_execution": "original CPython module/class/instance hooks",
                "calls": calls.copy(), "observation": {
                    "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {
                        "operation": "read the five native PEP 421 fields, named version fields and version slice",
                        "fixture": "original SysModuleTest lifecycle and check_impl_detail using Imre's name",
                        "assertions": "unchanged CPython TestCase assertions, dictionary lookup and hex arithmetic",
                        "method_sha256": hashlib.sha256(method_bytes(
                            Path(test_sys.__file__), "SysModuleTest", case._testMethodName)).hexdigest(),
                        "native_calls": len(calls), "calls": calls.copy(), "opt_level": level,
                    },
                }}

    return execute
