"""Compare native standard Base64 with the pinned CPython suite and optional Werkzeug wheels."""
import argparse
import ast
import base64
import binascii
from collections import Counter
import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeDefaultMethod, Toolchain, method_bytes

METHOD = "test_b64decode_padding_error"
HOSTED = ("test_b64encode", "test_b64decode", "test_b64decode_altchars", "test_b64decode_invalid_chars")
ERRORS = {"binascii.Error": binascii.Error, "AssertionError": AssertionError,
          "TypeError": TypeError, "ValueError": ValueError}


class PublicCodecs:
    def __init__(self, exe, root):
        self.exe, self.root, self.calls = exe, root, Counter()

    def native(self, op, data, altchars, flag):
        self.calls["native"] += 1
        payload, alphabet = self.root / "payload", self.root / "alphabet"
        payload.write_bytes(data.encode() if isinstance(data, str) else data)
        alphabet.write_bytes(altchars.encode() if isinstance(altchars, str) else altchars or b"")
        kind = "none" if altchars is None else "text" if isinstance(altchars, str) else "bytes"
        result = subprocess.run([self.exe, op, payload, kind, alphabet, str(int(flag))],
                                capture_output=True, text=True, check=True, timeout=30)
        lines = result.stdout.splitlines()
        if result.stderr or len(lines) != 2:
            raise AssertionError((result.stderr, lines))
        tag, value = lines
        if tag == "ok":
            return ast.literal_eval(value)
        raise ERRORS[tag](value)

    def b64encode(self, s, altchars=None):
        if type(s) is bytes and type(altchars) in (bytes, str, type(None)):
            return self.native("encode", s, altchars, False)
        self.calls["host"] += 1
        return base64.b64encode(s, altchars)

    def b64decode(self, s, altchars=None, validate=False):
        if type(s) in (bytes, str) and type(altchars) in (bytes, str, type(None)):
            return self.native("text" if isinstance(s, str) else "decode", s, altchars, validate)
        self.calls["host"] += 1
        return base64.b64decode(s, altchars, validate)

    def __getattr__(self, name):
        # The unchanged methods also call the standard/URL-safe aliases. Mark every
        # host call explicitly, including mutable buffer cases in the wrappers above.
        function = getattr(base64, name)
        def original(*args, **kwargs):
            self.calls["host"] += 1
            return function(*args, **kwargs)
        return original


def describe(call, *args):
    try:
        return "ok", call(*args)
    except (AssertionError, ValueError, TypeError) as exc:
        return type(exc), str(exc), exc.args


def compare_vectors(codec):
    alphabets = [None, b"-_", b"+/", b"$$", b"=+", b"\x00\xff", b"", b"!", b"abc", "-_", "π", "éx"]
    data = [b"", b"012", b"\xfb\xff", bytes(range(256))]
    encoded = [b"", b"A", b"abc", b"YQ==", b"YWJj", b"ab==c=", b"/w==", b"$$//", b"\x00\xff/+",
               b"A!\t=", "", "A", "YWJj", "!!!!", "--__", "é", "YWJj🐍"]
    count = 0
    for alt in alphabets:
        for payload in data:
            actual = describe(codec.b64encode, payload, alt)
            expected = describe(base64.b64encode, payload, alt)
            if actual != expected:
                raise AssertionError((payload, alt, expected, actual))
            count += 1
        for payload in encoded:
            for strict in (False, True):
                actual = describe(codec.b64decode, payload, alt, strict)
                expected = describe(base64.b64decode, payload, alt, strict)
                if actual != expected:
                    raise AssertionError((payload, alt, strict, expected, actual))
                count += 1
    return count


def authorization(auth, expected):
    results = {}
    for key, case in expected.items():
        row = {"header": case["header"]}
        try:
            value = auth.Authorization.from_header(case["header"])
            row["result"] = None if value is None else {
                "scheme": value.type, "parameters": value.parameters, "token": value.token,
                "to_header": value.to_header(),
            }
        except Exception as exc:
            row.update(exception=type(exc).__module__ + "." + type(exc).__name__, args=list(exc.args))
        if row != case:
            raise AssertionError((key, row, case))
        results[key] = row
    return results


def negative_controls(lang, dest):
    # Only the input literal changes in each counterfactual. The real
    # TestCase.assertRaises and its native b64decode operand are left intact.
    source = b"import unittest\nimport binascii\nimport base64\nclass BaseXYTestCase(unittest.TestCase):\n"
    source += method_bytes(HERE / "upstream/Lib/test/test_base64.py", "BaseXYTestCase", METHOD)
    source += b"\nif __name__ == '__main__':\n    unittest.main()\n"
    for index, (old, new, leaf, summary) in enumerate((
        (b"b'abc'", b"b'YWJj'", b"AssertionError: Error not raised by b64decode", b"FAILED (failures=1)"),
        (b", 'abc')", ", 'é')".encode(), b"string argument should contain only ASCII characters", b"FAILED (errors=1)"),
    )):
        if source.count(old) != 1:
            raise AssertionError("upstream padding-test control has changed")
        where = dest / f"negative-{index}"
        where.mkdir()
        binary = lang.build(source.replace(old, new), where)
        for command in ([binary], [sys.executable, "-IB", where / "case.py"]):
            result = subprocess.run(command, capture_output=True, timeout=60)
            if not result.returncode or leaf not in result.stderr or summary not in result.stderr:
                raise AssertionError((command, result.returncode, result.stderr))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--reachable-only", action="store_true")
    parser.add_argument("--wheelhouse", type=Path, help="measure unchanged Werkzeug auth using the seven locked offline wheels")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with pinned CPython 3.14.5 and its test package")
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import test_base64 as suite
    ref = json.loads((HERE / "reference-linux.json").read_bytes())
    discovered = [(case.id(), type(case).__name__) for case in cases(load_suite(manifest["module"]))]
    reports = []
    with tempfile.TemporaryDirectory() as td:
        root, auth = Path(td), None
        if args.wheelhouse:
            expected = json.loads((HERE / "werkzeug-basic.json").read_bytes())
            site = root / "site"
            site.mkdir()
            stage = runpy.run_path(str(HERE.parents[2] / "flask/gate.py"))["stage"]
            if stage(args.wheelhouse, site) != expected["wheels"]:
                raise ValueError("unexpected Werkzeug wheelhouse")
            sys.path.insert(0, str(site))
            from werkzeug.datastructures import auth
            source = Path(auth.__file__).resolve()
            if not source.is_relative_to(site) or hashlib.sha256(source.read_bytes()).hexdigest() != expected["module_sha256"]:
                raise ValueError("the original Authorization source has changed")
            authorization(auth, expected["cases"])
        for level in (0, 3):
            lang = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only)
            dest = root / str(level)
            dest.mkdir()
            child = dest / "default-method"
            child.mkdir()
            case = suite.BaseXYTestCase(METHOD)
            fixture = NativeDefaultMethod(lang, child, HERE / "upstream/Lib/test/test_base64.py",
                manifest["files"]["Lib/test/test_base64.py"],
                ("imre", "The original TestCase caught binascii.Error from b64decode for invalid bytes and ASCII text."))
            result = fixture.run(case)
            if result["outcome"] != "pass":
                raise AssertionError(result)
            negative_controls(lang, dest)
            observed = {(case.id(), 1): result}
            codec = PublicCodecs(lang.build((HERE / "worker.py").read_bytes(), dest), dest)
            with patch.object(suite, "base64", codec):
                for name in HOSTED:
                    case = suite.BaseXYTestCase(name)
                    codec.calls.clear()
                    r = case.run(unittest.TestResult())
                    if r.errors or r.failures or r.skipped or not codec.calls["native"]:
                        raise AssertionError((case.id(), r.failures, r.errors, r.skipped, codec.calls))
                    location = "mixed" if codec.calls["host"] else "imre"
                    observed[case.id(), 1] = {
                        "outcome": "pass", "roles": roles(body="cpython", assertions="cpython", operation=location, fixture="cpython"),
                        "evidence": {"operation": f"{codec.calls['native']} native public codec calls and {codec.calls['host']} CPython operations for other aliases or buffer types.",
                                     "fixture": "CPython ran the original fixture, helper methods and assertions."}}
            vectors = compare_vectors(codec)
            if auth:
                codec.calls.clear()
                with patch.object(auth, "base64", codec):
                    authorization(auth, expected["cases"])
                if codec.calls["host"] or not codec.calls["native"]:
                    raise AssertionError(codec.calls)
                print(f"O{level}: 16 unchanged Werkzeug observations, {codec.calls['native']} native base64 calls")
            reports.append(build_report(ref, level, discovered, observed))
            print(f"O{level}: {reports[-1]['counts']}; {vectors} differential public calls")
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()
