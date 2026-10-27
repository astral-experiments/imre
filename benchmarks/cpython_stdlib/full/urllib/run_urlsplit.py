"""Run original CPython URL parser tests and the wheel's WSGI environment producer."""
import argparse
import ast
import hashlib
import inspect
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
import urllib.parse

HERE = Path(__file__).resolve().parent
METHODS = ("test_urlsplit_scoped_IPv6", "test_urlsplit_attributes",
           "test_urlsplit_remove_unsafe_bytes", "test_urlsplit_strip_url",
           "test_invalid_bracketed_hosts", "test_splitting_bracketed_hosts",
           "test_urlsplit_normalization")
NATIVE_METHODS = {
    "test_invalid_bracketed_hosts": "urlsplit raising ValueError for invalid bracketed hosts",
    "test_quote_from_bytes": "quote_from_bytes returning escaped octets and rejecting a text operand",
}
TARGETS = ("/health", "/caf%C3%A9?q=%2F", "//first/second?x=y",
           "http://Example.com:8080/a%2Fb?x=1#f", "*")


def run_method(cls, name):
    result = cls(name).run()
    if not result.wasSuccessful() or result.testsRun != 1 or result.skipped or result.expectedFailures:
        raise AssertionError((name, result.errors, result.failures, result.skipped, result.expectedFailures))


def write_evidence(output, case, reference, report, native_methods):
    from suite_evidence import build_report, roles, write_reports
    from suite_inventory import cases, load_suite

    discovered = [(test.id(), "upstream") for test in cases(load_suite(reference["module"]))]
    if len(discovered) != report["reference_methods"] or reference["subtest_skips"] != report["reference_subtest_skips"]:
        raise ValueError("upstream discovery or skipped parameters differ from the checked reference")
    pair = []
    for level in (0, 3):
        normal, reachable = (report["modes"][f"{mode}-O{level}"] for mode in ("normal", "reachable"))
        if normal != reachable:
            raise ValueError(f"ordinary and reachable observations differ at O{level}")
        observations = {(case(name).id(), 1): observation
                        for name, observation in native_methods[f"normal-O{level}"].items()}
        for name in normal["host_methods_using_native_split"]:
            key = case(name).id(), 1
            if key in observations:
                continue
            mixed = name == "test_urlsplit_strip_url"
            observations[key] = {
                "outcome": "pass",
                "roles": roles(body="cpython", assertions="cpython", operation="mixed" if mixed else "imre", fixture="cpython"),
                "evidence": {
                    "operation": ("Native urlsplit and result fields with CPython urlunsplit." if mixed else
                                  "Native urlsplit and its observed result or error."),
                    "fixture": "CPython runs the original fixture, method and assertions with observed native results.",
                    "method_sha256": hashlib.sha256(inspect.getsource(getattr(case, name)).encode()).hexdigest(),
                },
            }
        pair.append(build_report(reference, level, discovered, observations))
    write_reports(output, pair, reference)


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
    parser.add_argument("--evidence", type=Path, help="write the paired per-method evidence for this run")
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    clang = shutil.which("clang")
    if not clang:
        parser.error("clang is required for the native method")
    from test import test_urlparse
    sys.path.insert(0, str(HERE.parents[1]))
    from suite_inventory import load_suite, passing_subtest_skips, verify_files, verify_interpreter_tests
    from suite_native import NativeDefaultMethod, Toolchain
    manifest = json.loads((HERE/"UPSTREAM.json").read_text())
    verify_files(HERE/"upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    case = test_urlparse.UrlParseTestCase
    full_count, skips, _ = passing_subtest_skips(load_suite(manifest["module"]))
    report = {"reference_methods": full_count, "reference_subtest_skips": skips, "modes": {}}
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
        source = "Lib/test/test_urlparse.py"
        native_methods = {}
        for mode in ("normal", "reachable"):
            for opt in ("0", "3"):
                tool = Toolchain(args.compiler, args.runtime, Path(clang), int(opt), mode == "reachable")
                observations = {}
                for name, operation in NATIVE_METHODS.items():
                    runner = NativeDefaultMethod(tool, directory, HERE / "upstream" / source,
                                                 manifest["files"][source], ("imre", operation))
                    observation = runner.run(case(name))
                    if observation["outcome"] != "pass":
                        raise RuntimeError(observation)
                    observations[name] = observation
                native_methods[f"{mode}-O{opt}"] = observations
                binary = directory / "runner"
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
                    "fully_native_isolated_methods": list(observations),
                    "host_methods_using_native_split": list(METHODS),
                    "werkzeug_make_environ": len(TARGETS),
                }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    if args.evidence:
        reference = json.loads((HERE / f"reference-urlparse-{sys.platform}.json").read_bytes())
        if reference["cpython_commit"] != manifest["commit"] or reference["module"] != manifest["module"]:
            raise ValueError("reference does not describe this pinned test module")
        write_evidence(args.evidence, case, reference, report, native_methods)
    print(f"{full_count} reference methods, {len(skips)} subtest skips, "
          f"{len(queries)} distinct native calls, {len(TARGETS)} Werkzeug environments per mode")


if __name__ == "__main__":
    main()
