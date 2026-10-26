"""Run unmodified CPython email date tests and upstream Werkzeug with native operations."""
import argparse
import ast
from collections import Counter
import datetime
import email.utils
import hashlib
import inspect
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


NATIVE_BODIES = ("test_naive_datetime", "test_aware_datetime",
                 "test_usegmt_with_naive_datetime_raises", "test_usegmt_with_non_utc_datetime_raises")


def native_body_source(cls):
    # CPython creates the original class attributes. Only serialize the fixture
    # values; keep each method and every assertion exactly as upstream wrote it.
    fields = ("datestring", "offsetstring", "naive_dt", "aware_dt")
    source = "import datetime\nfrom email import utils\nimport unittest\n\nclass DateTimeTests(unittest.TestCase):\n"
    for field in fields:
        source += f"    {field}: {'str' if isinstance(getattr(cls, field), str) else 'datetime.datetime'}\n"
    source += "    def __init__(self) -> None:\n        super().__init__()\n"
    for field in fields:
        source += f"        self.{field} = {getattr(cls, field)!r}\n"
    for name in NATIVE_BODIES:
        source += "\n" + inspect.getsource(getattr(cls, name))
    source += "\ncase = DateTimeTests()\n"
    for name in NATIVE_BODIES:
        source += f"case.{name}()\n"
    return source + 'print("four original methods passed")\n'


def original_tests(test_utils, test_email):
    loader = unittest.TestLoader()
    cases = list(loader.loadTestsFromTestCase(test_utils.DateTimeTests))
    # requires_mac_ver wraps the class in a callable. Its original fixture
    # and test methods still work when explicitly constructed; upstream's
    # default Linux discovery does not register these two cases.
    cases += [test_utils.FormatDateTests(n) for n in ("test_formatdate", "test_formatdate_with_localtime")]
    cases += [test_email.TestMiscellaneous(n) for n in (
        "test_formatdate", "test_formatdate_usegmt", "test_formatdate_localtime")]
    r = unittest.TestSuite(cases).run(unittest.TestResult())
    if not r.wasSuccessful() or r.skipped:
        raise AssertionError((r.errors, r.failures, r.skipped))
    return r.testsRun


def werkzeug_cases(http):
    parse_inputs = [None, "", "not a date", "Thu, 01 Jan 1970 00:00:00 GMT",
                    "Fri, 19-Nov-82 16:14:55 EST", "05 Feb 2003 13.47 -0000"]
    formats = [0, -0.0000005, datetime.date(2000, 2, 29), datetime.datetime(2001, 9, 23, 20, 10),
               datetime.datetime(2024, 3, 10, 7, tzinfo=datetime.timezone(datetime.timedelta(hours=5))),
               time.gmtime(1672531200)]
    return {
        "parse_date": [v.isoformat() if (v := http.parse_date(x)) is not None else None for x in parse_inputs],
        "http_date": [http.http_date(x) for x in formats],
    }


class Native:
    def __init__(self, binary):
        self.binary, self.calls = binary, Counter()

    def invoke(self, method, *values):
        self.calls[method] += 1
        p = subprocess.run([self.binary, method, *map(str, values)],
                           check=True, capture_output=True, text=True, timeout=10)
        assert not p.stderr
        kind, data = ast.literal_eval(p.stdout)
        if kind == "ok":
            return data
        raise {"ValueError": ValueError, "OverflowError": OverflowError}[kind](data)

    def parse(self, data):
        # The worker returns an actual native datetime's public ISO spelling.
        # Rebuild a CPython value for the unmodified host equality and replace() calls.
        return datetime.datetime.fromisoformat(self.invoke("parse", data))

    def fmt(self, dt, usegmt=False):
        offset = dt.utcoffset()
        us = None if offset is None else offset // datetime.timedelta(microseconds=1)
        return self.invoke("datetime", dt.year, dt.month, dt.day, dt.hour, dt.minute,
                           dt.second, dt.microsecond, us, int(usegmt))

    def timestamp(self, timeval=None, localtime=False, usegmt=False):
        return self.invoke(type(timeval).__name__, timeval, int(localtime), int(usegmt))

    def patched(self):
        return patch.multiple(email.utils, parsedate_to_datetime=self.parse,
                              format_datetime=self.fmt, formatdate=self.timestamp)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime", "wheelhouse", "output"):
        p.add_argument("--"+name, type=Path, required=True)
    p.add_argument("--evidence", type=Path, help="write the paired standard per-occurrence ledger")
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        p.error("use CPython 3.14.5 with its test package")
    from test.test_email import test_utils, test_email
    sys.path.insert(0, str(HERE.parents[1]))
    from suite_inventory import verify_files, verify_interpreter_tests
    manifest = json.loads((HERE/"UPSTREAM.json").read_text())
    verify_files(HERE/"upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    report = {"host_original_methods": original_tests(test_utils, test_email),
              "outside_linux_discovery": ["FormatDateTests.test_formatdate", "FormatDateTests.test_formatdate_with_localtime"],
              "modes": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        sys.path.insert(0, str(HERE.parents[2]/"flask"))
        import gate
        site = work/"site"
        site.mkdir()
        gate.stage(args.wheelhouse, site)
        sys.path.insert(0, str(site))
        from werkzeug import http
        assert Path(http.__file__).resolve().is_relative_to(site)
        report["werkzeug_http_sha256"] = hashlib.sha256(Path(http.__file__).read_bytes()).hexdigest()
        expected = werkzeug_cases(http)
        report["werkzeug"] = expected
        unused = work/"unused_declarations.py"
        unused.write_text("from email import message, parser, policy\n")
        whole = subprocess.run([args.compiler, "check", unused], capture_output=True)
        reachable = subprocess.run([args.compiler, "check", unused, "--reachable-only"], capture_output=True)
        assert whole.returncode != 0 and reachable.returncode == 0, (whole.stderr, reachable.stderr)
        body = work/"original_bodies.py"
        body.write_text(native_body_source(test_utils.DateTimeTests))
        assert subprocess.check_output([sys.executable, "-IB", body], text=True).strip() == "four original methods passed"
        report["native_bodies_host_fixture"] = list(NATIVE_BODIES)
        for mode in ("normal", "reachable"):
            for opt in ("0", "3"):
                binary = work/"dates"
                cmd = [args.compiler, "build", HERE/"worker.py", "--no-cache", "--runtime", args.runtime,
                       "--opt-level", opt, "-o", binary]
                if mode == "reachable":
                    cmd.append("--reachable-only")
                built = subprocess.run(cmd, text=True, capture_output=True)
                if built.returncode:
                    raise AssertionError(built.stderr)
                worker = Native(binary)
                with worker.patched():
                    assert original_tests(test_utils, test_email) == report["host_original_methods"]
                    assert werkzeug_cases(http) == expected
                report["modes"][f"{mode}-O{opt}"] = dict(sorted(worker.calls.items()))
                cmd[2] = body
                built = subprocess.run(cmd, text=True, capture_output=True)
                if built.returncode:
                    raise AssertionError(built.stderr)
                assert subprocess.check_output([binary], text=True).strip() == "four original methods passed"
    args.output.write_text(json.dumps(report, indent=2)+"\n")
    if args.evidence:
        from suite_evidence import build_report, roles, validate_report
        from suite_inventory import cases, load_suite
        reference = json.loads((HERE/"reference-linux.json").read_text())
        discovered = [(c.id(), "default Linux unittest discovery") for c in cases(load_suite("test.test_email"))]
        observations = {}
        for key, _variant in discovered:
            cls, test = key.rsplit(".", 2)[1:]
            if cls == "DateTimeTests" and test in NATIVE_BODIES:
                observations[(key, 1)] = {"outcome": "pass",
                    "roles": roles(body="imre", assertions="imre", operation="imre", fixture="cpython"),
                    "evidence": {"operation": "Unchanged method with native TestCase assertions, calendar objects and email.utils.",
                                 "fixture": "The original CPython class evaluated the datetime and string fixture attributes."}}
            elif ((cls == "DateTimeTests" and key.startswith("test.test_email.test_utils.")) or
                  (cls == "TestMiscellaneous" and test in ("test_formatdate", "test_formatdate_localtime", "test_formatdate_usegmt"))):
                observations[(key, 1)] = {"outcome":"pass",
                    "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {"operation": "The original method called the native RFC date worker. CPython provided comparison oracles, including parsedate for the formatdate roundtrips.",
                                 "fixture": "Unchanged CPython datetime, clock and TestCase setup."}}
        exceptions = {}
        for test_id, variant in discovered:
            ref = reference["reference"][test_id]
            if not isinstance(ref, dict) or "skipped" not in ref:
                continue
            cls, name = test_id.rsplit(".", 2)[1:]
            method = getattr(getattr(test_email, cls), name)
            line = inspect.getsourcelines(method)[1]
            for opt in (0, 3):
                exceptions[test_id, 1, opt] = {
                    "kind": "upstream-skip", "reason": ref["skipped"],
                    "condition": ("upstream unconditional skip for email5 compatibility"
                                  if name == "test_utf8_input_no_charset" else "cpu resource disabled"),
                    "source": f"Lib/test/test_email/test_email.py:{line}",
                    "configuration": "upstream default Linux regrtest resources",
                    "platform": "linux", "fixture_variant": variant,
                }
        pair = [build_report(reference, opt, discovered, observations,
                             exceptions={k: v for k, v in exceptions.items() if k[2] == opt})
                for opt in (0, 3)]
        for entry in pair:
            validate_report(entry, reference)
        args.evidence.write_text(json.dumps(pair, indent=2)+"\n")
    print(report)


if __name__ == "__main__":
    main()
