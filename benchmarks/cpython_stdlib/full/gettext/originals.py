"""Repeat the passing, unchanged gettext methods recorded in the Linux ledger."""
import argparse
from contextlib import chdir
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from test import test_gettext

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import validate_pair
from suite_inventory import cases
from suite_native import Toolchain, method_bytes, run_in_fixtures

SOURCE = HERE / "upstream/Lib/test/test_gettext.py"
DOMAIN_FIXTURE = b'''import unittest
import gettext

class NativeCase(unittest.TestCase):
    localedir = "."

    def _(self, message: str) -> str:
        return gettext.gettext(message)
'''


def domain_method(tool, directory, name, body):
    """Run in the original class and instance hooks with the same public bindings."""
    method = name.rsplit(".", 1)[1]
    entry = DOMAIN_FIXTURE + body + (
        '\ngettext.bindtextdomain("gettext", ".")\ngettext.textdomain("gettext")\n'
        f'case = NativeCase()\ncase.{method}()\nprint("completed {name}")\n'
    ).encode()
    binary = tool.build(entry, directory)

    def execute(case, original):
        result = subprocess.run([binary], capture_output=True, text=True, timeout=60)
        if (result.returncode, result.stdout, result.stderr) != (0, f"completed {name}\n", ""):
            raise AssertionError((result.args, result.returncode, result.stdout, result.stderr))
        return {"operation": "native domain function under original CPython hooks"}

    return execute


def plural_method(tool, directory, method, body, imports_and_class):
    """Run the original default TestCase lifecycle and assertions inside Imre."""
    entry = imports_and_class + body + b'\nif __name__ == "__main__":\n    unittest.main()\n'
    binary = tool.build(entry, directory)
    result = subprocess.run([binary, "-v"], cwd=directory, capture_output=True, text=True, timeout=60)
    name = f"__main__.PluralFormsInternalTestCase.{method}"
    expected = f"{method} ({name}) ... ok\n\n{'-' * 70}\nRan 1 test in <time>s\n\nOK\n"
    stderr = re.sub(r"Ran 1 test in \d+\.\d+s", "Ran 1 test in <time>s", result.stderr)
    if (result.returncode, result.stdout, stderr) != (0, "", expected):
        raise AssertionError((result.args, result.returncode, result.stdout, result.stderr))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", type=Path, required=True)
    parser.add_argument("--reachable-only", action="store_true")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or not sys.platform.startswith("linux"):
        parser.error("use the pinned CPython 3.14.5 Linux reference")
    data = SOURCE.read_bytes()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    assert blob == json.loads((HERE / "UPSTREAM.json").read_text())["files"]["Lib/test/test_gettext.py"]
    assert Path(test_gettext.__file__).read_bytes() == data
    lines = data.splitlines(keepends=True)
    # The helper needs these two original imports. The suite also imports
    # unittest.mock, which binds unittest again in the full module.
    required = (b"import gettext\n", b"import unittest\n")
    header = b"class PluralFormsInternalTestCase(unittest.TestCase):\n"
    assert all(lines.count(line) == 1 for line in (*required, header))
    prefix = b"".join(line for line in lines if line in required) + b"\n" + header
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports = json.loads((HERE / "evidence-linux.json").read_text())
    validate_pair(reports, reference)
    def suite():
        return unittest.defaultTestLoader.loadTestsFromModule(test_gettext)

    compiler, runtime, clang = args.compiler.resolve(), args.runtime.resolve(), args.clang.resolve()
    assert sorted(case.id() for case in cases(suite())) == sorted(reference["reference"])
    with tempfile.TemporaryDirectory() as temp, chdir(temp):
        for report in reports:
            level = report["opt_level"]
            tool = Toolchain(compiler, runtime, clang, level,
                             reachable_only=args.reachable_only)
            callbacks = {}
            native = 0
            for row in report["records"]:
                if row["outcome"] != "pass":
                    continue
                name = row["id"]
                owner, method = name.split(".")[-2:]
                body = method_bytes(SOURCE, owner, method)
                assert hashlib.sha256(body).hexdigest() == row["evidence"]["method_sha256"]
                directory = Path(temp) / str(level) / name
                directory.mkdir(parents=True)
                if row["classification"] == "fully-native":
                    assert owner == "PluralFormsInternalTestCase"
                    plural_method(tool, directory, method, body, prefix)
                    native += 1
                else:
                    assert row["classification"] == "native-body-host-fixture" and owner == "GettextTestCase2"
                    callbacks[name] = domain_method(tool, directory, name, body)
            results, failures, skips = run_in_fixtures(suite(), callbacks)
            assert set(results) == set(callbacks) and not failures and not skips, (results, failures, skips)
            print(f"O{level}: {native} fully native, {len(results)} with CPython fixtures")


if __name__ == "__main__":
    main()
