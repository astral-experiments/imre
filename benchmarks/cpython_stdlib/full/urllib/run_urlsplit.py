"""Run original CPython URL parser tests and the wheel's WSGI environment producer."""
import argparse
import ast
import hashlib
import inspect
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import urllib.parse

HERE = Path(__file__).resolve().parent
METHODS = ("test_urlsplit_scoped_IPv6", "test_urlsplit_attributes",
           "test_urlsplit_remove_unsafe_bytes", "test_urlsplit_strip_url",
           "test_invalid_bracketed_hosts", "test_splitting_bracketed_hosts",
           "test_urlsplit_normalization")
TARGETS = ("/health", "/caf%C3%A9?q=%2F", "//first/second?x=y",
           "http://Example.com:8080/a%2Fb?x=1#f", "*")


def run_method(cls, name):
    result = cls(name).run()
    if not result.wasSuccessful() or result.testsRun != 1:
        raise AssertionError((name, result.errors, result.failures))


def werkzeug_environ(serving):
    # The unchanged method receives the fields that the protocol parser normally sets.
    handler = object.__new__(serving.WSGIRequestHandler)
    handler.server = SimpleNamespace(ssl_context=None, multithread=False,
        multiprocess=False, server_address=("127.0.0.1", 8080), _server_version="Werkzeug")
    handler.client_address = ("127.0.0.1", 12345)
    handler.connection = object()
    handler.command, handler.request_version = "GET", "HTTP/1.1"
    handler.rfile, handler.headers = io.BytesIO(), {"Host": "original.test"}
    outputs = []
    for handler.path in TARGETS:
        env = handler.make_environ()
        outputs.append(tuple(env[key] for key in ("PATH_INFO", "QUERY_STRING", "HTTP_HOST", "RAW_URI")))
    return outputs


def key(args, kwargs):
    return args, tuple(kwargs.items())


class Observed(tuple):
    """Expose native result data; eager native port failures are replayed on property access."""

    def __new__(cls, info):
        obj = super().__new__(cls, info[0])
        obj.scheme, obj.netloc, obj.path, obj.query, obj.fragment = obj
        obj.username, obj.password, obj.hostname, obj.url, obj._port, obj.error = info[1:]
        return obj

    @property
    def port(self):
        if self.error:
            raise ValueError(self.error)
        return self._port

    def geturl(self):
        return self.url


def native_input(queries):
    source = "from urllib.parse import urlsplit, SplitResult, SplitResultBytes\n"
    template = """
def observeTYPE(p: TYPE) -> None:
    port: int | None = None
    reason = ""
    try:
        port = p.port
    except ValueError as error:
        reason = str(error)
    print(((p.scheme,p.netloc,p.path,p.query,p.fragment),p.username,p.password,p.hostname,p.geturl(),port,reason))
"""
    for cls in ("SplitResult", "SplitResultBytes"):
        source += template.replace("TYPE", cls)
    for args, kwargs in queries:
        shape = "SplitResultBytes" if isinstance(args[0], bytes) else "SplitResult"
        params = [*(repr(v) for v in args), *(f"{k}={v!r}" for k, v in kwargs)]
        source += (f"try:\n    observe{shape}(urlsplit({', '.join(params)}))\n"
                   'except ValueError as error:\n    print(("error", str(error)))\n')
    return source


def build(compiler, runtime, source, output, mode, opt):
    command = [compiler, "build", source, "--module-root", source.parent, "--no-cache",
               "--runtime", runtime, "--opt-level", opt, "-o", output]
    if mode == "reachable":
        command.append("--reachable-only")
    p = subprocess.run(command, text=True, capture_output=True, timeout=180)
    if p.returncode:
        raise RuntimeError(p.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime", "wheelhouse", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    from test import test_urlparse
    sys.path.insert(0, str(HERE.parents[1]))
    from suite_inventory import verify_files, verify_interpreter_tests
    manifest = json.loads((HERE/"UPSTREAM.json").read_text())
    verify_files(HERE/"upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    case = test_urlparse.UrlParseTestCase
    full = unittest.defaultTestLoader.loadTestsFromModule(test_urlparse).run(unittest.TestResult())
    assert full.wasSuccessful()
    skips = [(sub.test_case.id(), {
        name: (value.__name__ if callable(value) else value)
        for name, value in sub.params.items()
    }, reason) for sub, reason in full.skipped]
    report = {"reference_methods": full.testsRun, "reference_subtest_skips": skips, "modes": {}}
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        sys.path.insert(0, str(HERE.parents[2]/"flask"))
        import gate
        site = directory / "site"
        site.mkdir()
        report["wheels"] = gate.stage(args.wheelhouse, site)
        sys.path.insert(0, str(site))
        from werkzeug import serving
        assert Path(serving.__file__).resolve().is_relative_to(site)
        report["serving_sha256"] = hashlib.sha256(Path(serving.__file__).read_bytes()).hexdigest()
        original = urllib.parse.urlsplit
        queries = {}
        def record(*args, **kwargs):
            queries[key(args, kwargs)] = None
            return original(*args, **kwargs)
        with patch.object(urllib.parse, "urlsplit", record):
            for name in METHODS:
                run_method(case, name)
        with patch.object(serving, "urlsplit", record):
            expected_env = werkzeug_environ(serving)
        report["werkzeug_environments"] = expected_env
        report["distinct_split_calls"] = len(queries)
        calls = directory / "calls.py"
        calls.write_text(native_input(queries))
        method = directory / "native_case.py"
        name = "test_invalid_bracketed_hosts"
        method.write_text("import unittest\nimport urllib.parse\n\nclass UrlParseTestCase(unittest.TestCase):\n"
                          + inspect.getsource(getattr(case, name))
                          + '\nif __name__ == "__main__":\n    unittest.main()\n')
        for mode in ("normal", "reachable"):
            for opt in ("0", "3"):
                binary = directory / "runner"
                build(args.compiler, args.runtime, method, binary, mode, opt)
                run = subprocess.run([binary, "-v"], capture_output=True, text=True, check=True)
                assert name in run.stderr and "Ran 1 test" in run.stderr and run.stderr.rstrip().endswith("OK")
                build(args.compiler, args.runtime, calls, binary, mode, opt)
                rows = subprocess.check_output([binary], text=True).splitlines()
                results = dict(zip(queries, map(ast.literal_eval, rows), strict=True))
                def replay(*args, **kwargs):
                    info = results[key(args, kwargs)]
                    if info[0] == "error":
                        raise ValueError(info[1])
                    return Observed(info)
                with patch.object(urllib.parse, "urlsplit", replay):
                    for test in METHODS:
                        run_method(case, test)
                with patch.object(serving, "urlsplit", replay):
                    assert werkzeug_environ(serving) == expected_env
                report["modes"][f"{mode}-O{opt}"] = {
                    "fully_native_isolated_method": name,
                    "host_methods_using_native_split": list(METHODS),
                    "werkzeug_make_environ": len(TARGETS),
                }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"{full.testsRun} reference methods, {len(skips)} subtest skips, "
          f"{len(queries)} distinct native calls, {len(TARGETS)} Werkzeug environments per mode")


if __name__ == "__main__":
    main()
