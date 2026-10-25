"""Observe the original test_ioencoding_nonascii decorator, child and bytes."""

import hashlib
import json
import locale
import os
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import run_in_fixtures

CASE_ID = "test.test_sys.SysModuleTest.test_ioencoding_nonascii"


def settings():
    return {
        "LC_CTYPE": locale.setlocale(locale.LC_CTYPE),
        "utf8_mode": sys.flags.utf8_mode,
        "filesystem_encoding": sys.getfilesystemencoding(),
        "preferred_encoding": locale.getpreferredencoding(False),
        "environment": {key: os.environ.get(key) for key in
                        ("LANG", "LC_ALL", "LC_CTYPE", "PYTHONUTF8", "PYTHONIOENCODING")},
    }


def observer(module, toolchain, folder, method_sha256):
    def execute(case, original):
        children = []

        class Child(subprocess.Popen):
            def __init__(self, argv, **kwargs):
                if len(argv) != 3 or argv[:2] != [sys.executable, "-c"]:
                    raise ValueError(f"unexpected child: {argv!r}")
                data = argv[2].encode()
                self.record = {"source": argv[2], "sha256": hashlib.sha256(data).hexdigest(),
                               "PYTHONIOENCODING": kwargs["env"]["PYTHONIOENCODING"]}
                children.append(self.record)
                if toolchain:
                    folder.mkdir()
                    argv = [toolchain.build(data, folder)]
                super().__init__(argv, **kwargs)

            def communicate(self, *args, **kwargs):
                out, err = super().communicate(*args, **kwargs)
                self.record.update(stdout_hex=out.hex(), returncode=self.returncode)
                return out, err

        with patch.object(module, "subprocess", SimpleNamespace(Popen=Child, PIPE=subprocess.PIPE)):
            original()
        child, = children
        if child["returncode"]:
            raise ValueError(child)
        native = toolchain is not None
        proof = {"operation": "child prints the original non-ASCII value through stdout",
                 "fixture": "original CPython locale/subprocess decorators, child environment and pipes",
                 "assertions": "original CPython comparison with os.fsencode",
                 "method_sha256": method_sha256, "child": child}
        if native:
            proof.update(native_calls=1, child_sha256=child["sha256"], opt_level=toolchain.level)
        return {"execution": "host-driven-native" if native else "reference",
                "body": "CPython; child source: Imre" if native else "CPython",
                "assertions": "CPython", "fixture_execution": "CPython module/class/instance hooks",
                "child_sha256": child["sha256"],
                "observation": {"roles": roles(body="cpython", assertions="cpython",
                                               operation="imre" if native else "cpython", fixture="cpython"),
                                "evidence": proof}}
    return execute


def audit(path, module, toolchain, folder, method_sha256, commit):
    """Run both subjects with the same CPython flags and untouched skip decorator."""
    rows = {}
    for subject, chain in (("cpython", None), ("imre", toolchain)):
        values, failures, skips = run_in_fixtures(load_suite(module.__name__), {
            CASE_ID: observer(module, chain, folder, method_sha256)})
        if failures:
            raise AssertionError(failures)
        if skips:
            (case_id, reason), = skips
            assert case_id == CASE_ID and not values
            rows[subject] = {"status": "skip", "reason": reason, "child": None}
        else:
            record, = values[CASE_ID]
            rows[subject] = {"status": "pass", **record["observation"]}
    if rows["cpython"]["status"] != rows["imre"]["status"]:
        raise AssertionError(rows)
    if rows["imre"]["status"] == "pass" and rows["imre"]["evidence"]["child"] != rows["cpython"]["evidence"]["child"]:
        raise AssertionError(rows)
    result = {"id": CASE_ID, "occurrence": 1, "cpython_commit": commit, "opt_level": toolchain.level,
              "method_sha256": method_sha256, "host": settings(), "observations": rows}
    path.write_text(json.dumps(result, indent=2) + "\n")
