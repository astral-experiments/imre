"""Run pinned urllib tests and real Werkzeug helpers with observed native quote/unquote results."""
import argparse
import ast
from collections import Counter
from contextlib import ExitStack
import hashlib
import inspect
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch
import urllib.parse

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from run_urlsplit import build, run_method
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, reference_cases, roles, write_reports
from suite_inventory import verify_files, verify_interpreter_tests
from suite_native import method_bytes

METHODS = {
    "QuotingTests": ("test_never_quote", "test_safe", "test_default_quoting", "test_quoting_space",
                     "test_quote_bytes", "test_quote_with_unicode"),
    "UnquotingTests": ("test_unquoting", "test_unquoting_badpercent", "test_unquoting_parts",
                       "test_unquoting_plus", "test_unquote_with_unicode", "test_unquoting_with_bytes_input"),
}
NATIVE_BODIES = ("test_quote_bytes", "test_quote_with_unicode", "test_unquote_with_unicode", "test_unquoting_with_bytes_input")


def wheel_cases(http):
    return {
        "dump_cookie": [http.dump_cookie("sid", "value", path=path)
                        for path in ("/", "/snow 雪/%2F+x=y", "/%!$&'()*+,/:=@", "/* café/;#?", None)],
        "parse_dict_header": [http.parse_dict_header(value) for value in (
            "name*=UTF-8''caf%C3%A9, flag", "name*=ISO-8859-1'en'caf%E9",
            "name*=us-ascii''%ff+%2b", "name*=utf-8''bad%ZZ", "name*=koi8-r''%FF")],
        "parse_options_header": [http.parse_options_header(value) for value in (
            "attachment; filename*=UTF-8''snow-%E9%9B%AA.txt",
            "text/plain; title*=iso-8859-1'en'%E9+space", "text/plain; n*=ascii''%FF",
            "attachment; filename*0*=utf-8''a%2F; filename*1*=%2Bb", "text/plain; n*=bogus''%FF")],
    }


def spelling(name, args, kwargs):
    parts = [*(repr(arg) for arg in args), *(f"{key}={value!r}" for key, value in kwargs.items())]
    return f"{name}({', '.join(parts)})"


def redirect(target, handler):
    stack = ExitStack()
    for name in ("quote", "unquote"):
        def proxy(*args, _name=name, **kwargs):
            return handler(_name, args, kwargs)
        stack.enter_context(patch.object(target, name, proxy))
    return stack


def evidence(reference, test_module, calls, output):
    fixture = "default Linux regrtest resources"
    observations = {}
    exceptions = {}
    for (key, occurrence), outcome in reference_cases(reference).items():
        cls, name = key.rsplit(".", 2)[-2:]
        if isinstance(outcome, dict) and "skipped" in outcome:
            _, line = inspect.getsourcelines(getattr(getattr(test_module, cls), name))
            for level in (0, 3):
                exceptions[key, occurrence, level] = {"kind": "upstream-skip", "reason": outcome["skipped"],
                    "condition": outcome["skipped"], "source": f"Lib/test/test_urllib.py:{line}",
                    "configuration": fixture, "platform": "linux", "fixture_variant": fixture}
        elif key in calls:
            native = name in NATIVE_BODIES
            loc = roles(body="imre" if native else "cpython", assertions="imre" if native else "cpython",
                        operation="imre" if native else "mixed", fixture="mixed" if native else "cpython")
            observations[key, occurrence] = {"outcome": "pass", "roles": loc, "evidence": {
                "operation": f"Native public quote/unquote results for {dict(calls[key])} calls. "
                             + ("The native test body also calls quote_from_bytes." if name == "test_quote_bytes"
                                else "" if native else "Other urllib entry points run on CPython."),
                "fixture": ("The native harness supplies the isolated TestCase entry and its two imports."
                            if native else "CPython runs the original test and assertions."),
            }}
    discovered = [(key, fixture) for key, _ in reference_cases(reference)]
    reports = [build_report(reference, level, discovered, observations,
                            exceptions={k: v for k, v in exceptions.items() if k[2] == level}) for level in (0, 3)]
    write_reports(output, reports, reference)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("compiler", "runtime", "wheelhouse", "output", "evidence"):
        parser.add_argument("--" + arg, type=Path, required=True)
    opts = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM-urllib.json").read_bytes())
    reference = json.loads((HERE / "reference-urllib-linux.json").read_bytes())
    if ("v" + sys.version.split()[0], reference["cpython_commit"], reference["module"]) != (
            manifest["tag"], manifest["commit"], manifest["module"]):
        raise ValueError("the reference and interpreter must match the vendored suite")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import test_urllib
    sys.path.insert(0, str(HERE.parents[2] / "flask"))
    import gate

    cases = [getattr(test_urllib, cls)(name) for cls, names in METHODS.items() for name in names]
    calls = {}
    answers = {}
    original = {name: getattr(urllib.parse, name) for name in ("quote", "unquote")}
    current = Counter()

    def record(name, args, kwargs):
        key = spelling(name, args, kwargs)
        current[name] += 1
        try:
            value = original[name](*args, **kwargs)
        except (TypeError, ValueError, LookupError) as exc:
            answers[key] = type(exc), exc.args, str(exc)
            raise
        answers[key] = str, (value,), value
        return value

    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp)
        site = directory / "site"
        site.mkdir()
        wheels = gate.stage(opts.wheelhouse, site)
        sys.path.insert(0, str(site))
        from werkzeug import http
        if not Path(http.__file__).resolve().is_relative_to(site):
            raise RuntimeError("Werkzeug must come from the verified wheelhouse")
        werkzeug_hash = hashlib.sha256(Path(http.__file__).read_bytes()).hexdigest()
        with redirect(urllib.parse, record):
            for case in cases:
                current = calls[case.id()] = Counter()
                run_method(type(case), case._testMethodName)
                if not current:
                    raise ValueError(f"no product operation observed in {case.id()}")
        current = Counter()
        with redirect(http, record):
            expected_wheel = wheel_cases(http)
        wheel_calls = dict(current)
        # Capture all single-octet encodings and a full round trip independently of upstream methods.
        record("quote", (bytes(range(256)),), {"safe": b""})
        record("unquote", (original["quote"](bytes(range(256)), safe=""),), {"encoding": "latin-1"})
        program = ["from urllib.parse import quote, unquote"]
        expected = []
        for expr, (kind, args, value) in answers.items():
            if kind is str:
                program.append(f"print(repr({expr}))")
            else:
                program.extend(["try:", f"    print(repr({expr}))", f"except {kind.__name__} as error:",
                                "    print(repr(str(error)))", "    print(repr(error.args))"])
            expected.append(value)
            if kind is not str:
                expected.append(args)
        source = directory / "calls.py"
        source.write_text("\n".join(program) + "\n")
        test_path = HERE / "upstream/Lib/test/test_urllib.py"
        method_entry = directory / "methods.py"
        entry = bytearray(b"import unittest\nimport urllib.parse\n")
        for cls, names in METHODS.items():
            entry.extend(f"class {cls}(unittest.TestCase):\n".encode())
            for name in names:
                if name in NATIVE_BODIES:
                    entry.extend(method_bytes(test_path, cls, name) + b"\n")
        method_entry.write_bytes(entry + b"\nif __name__ == '__main__':\n    unittest.main()\n")
        modes = {}
        for mode in ("normal", "reachable"):
            for opt in ("0", "3"):
                binary = directory / "runner"
                build(opts.compiler, opts.runtime, source, binary, mode, opt)
                rows = list(map(ast.literal_eval, subprocess.check_output([binary], text=True).splitlines()))
                if rows != expected:
                    raise AssertionError([(a, b) for a, b in zip(rows, expected, strict=True) if a != b][:4])
                observed = iter(rows)
                results = {}
                for expr, (kind, args, _) in answers.items():
                    value = next(observed)
                    results[expr] = value if kind is str else kind(*next(observed))

                def replay(name, args, kwargs):
                    result = results[spelling(name, args, kwargs)]
                    if isinstance(result, BaseException):
                        raise result
                    return result

                with redirect(urllib.parse, replay):
                    for case in cases:
                        run_method(type(case), case._testMethodName)
                with redirect(http, replay):
                    if wheel_cases(http) != expected_wheel:
                        raise AssertionError("Werkzeug results differ")
                build(opts.compiler, opts.runtime, method_entry, binary, mode, opt)
                ran = subprocess.run([binary], capture_output=True, text=True, check=True)
                if f"Ran {len(NATIVE_BODIES)} tests" not in ran.stderr + ran.stdout or "OK" not in ran.stderr + ran.stdout:
                    raise AssertionError((ran.stdout, ran.stderr))
                modes[f"{mode}-O{opt}"] = {"distinct_native_calls": len(answers),
                                           "isolated_native_bodies": list(NATIVE_BODIES)}
    report = {"upstream_commit": manifest["commit"], "original_calls": calls, "wheel_calls": wheel_calls,
              "werkzeug_results": expected_wheel, "werkzeug_http_sha256": werkzeug_hash,
              "wheels": wheels, "modes": modes}
    opts.output.write_text(json.dumps(report, indent=2) + "\n")
    evidence(reference, test_urllib, calls, opts.evidence)


if __name__ == "__main__":
    main()
