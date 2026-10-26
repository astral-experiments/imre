"""Run the original CPython TCP client methods against a separately compiled server."""
import argparse
import ast
from contextlib import contextmanager
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import textwrap
import unittest

HERE = Path(__file__).resolve().parent
UPSTREAM = HERE / "upstream/Lib/test/test_httpservers.py"
METHODS = (
    "test_command", "test_request_line_trimming", "test_version_bogus",
    "test_version_digits", "test_version_signs_and_underscores",
    "test_major_version_number_too_long", "test_minor_version_number_too_long",
    "test_version_none_get", "test_version_none", "test_version_invalid",
    "test_send_blank", "test_header_close", "test_header_keep_alive", "test_handler",
    "test_return_header_keep_alive", "test_internal_key_error", "test_return_custom_status",
    "test_return_explain_error", "test_latin1_header", "test_error_content_length",
)


def original_tests():
    for name, expected in json.loads((HERE / "UPSTREAM.json").read_text())["files"].items():
        data = (HERE / "upstream" / name).read_bytes()
        assert hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == expected, name
    from test import support
    support.use_resources = ["network"]
    spec = importlib.util.spec_from_file_location("test.test_httpservers", UPSTREAM)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.BaseHTTPServerTestCase


def program():
    source = UPSTREAM.read_text()
    tree = ast.parse(source)
    cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "BaseHTTPServerTestCase")
    handler = next(n for n in cls.body if isinstance(n, ast.ClassDef) and n.name == "request_handler")
    lines = source.splitlines(keepends=True)
    methods = [textwrap.indent(textwrap.dedent("".join(lines[n.lineno - 1:n.end_lineno])), "    ")
               for n in handler.body if isinstance(n, ast.FunctionDef) and n.name.startswith("do_")]
    return ("""import sys
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    default_request_version = "HTTP/1.1"
""" + "\n".join(methods) + """

class Mutant(Handler):
    def do_TEST(self) -> None:
        self.send_response(200)
        self.send_header("Connection", "close")
        self.end_headers()


if sys.argv[1] == "wrong":
    server = HTTPServer(("127.0.0.1", 0), Mutant)
else:
    server = HTTPServer(("127.0.0.1", 0), Handler)
with server:
    print(server.server_address[1])
    sys.stdout.flush()
    server.handle_request()
""")


@contextmanager
def listener(command, wrong=False):
    with tempfile.TemporaryFile() as errors:
        with subprocess.Popen([*command, "wrong" if wrong else "normal"], stdout=subprocess.PIPE, stderr=errors) as child:
            try:
                if not select.select([child.stdout], [], [], 10)[0]:
                    raise AssertionError("listener did not publish its port")
                line = child.stdout.readline()
                if not line:
                    errors.seek(0)
                    raise AssertionError(errors.read().decode())
                yield int(line)
                if child.wait(timeout=8):
                    errors.seek(0)
                    raise AssertionError(errors.read().decode())
            finally:
                if child.poll() is None:
                    child.kill()


def run_case(upstream, name, command, wrong=False):
    with listener(command, wrong) as port:
        # The original test owns every request and assertion. Only the listener
        # fixture crosses the process boundary into the compiled implementation.
        class Wire(upstream):
            def setUp(self):
                self.HOST, self.PORT = "127.0.0.1", port
                self.con = http.client.HTTPConnection(self.HOST, port, timeout=5)
                self.con.connect()

            def tearDown(self):
                self.con.close()
        case = Wire(name)
        result = unittest.TestResult()
        case.run(result)
        if result.errors or result.skipped or len(result.failures) != int(wrong):
            raise AssertionError((name, result.errors, result.failures, result.skipped))
        if wrong and "200 !=" not in result.failures[0][1]:
            raise AssertionError(result.failures)


def main():
    options = argparse.ArgumentParser(description=__doc__)
    options.add_argument("--compiler", required=True, type=Path)
    options.add_argument("--runtime", required=True, type=Path)
    options.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    options.add_argument("--clang", default="clang")
    args = options.parse_args()
    upstream = original_tests()
    with tempfile.TemporaryDirectory(prefix="imre-http-") as tmp:
        root = Path(tmp)
        source = root / "serve.py"
        source.write_text(program())
        for name in METHODS:
            run_case(upstream, name, [sys.executable, str(source)])
        run_case(upstream, "test_handler", [sys.executable, str(source)], wrong=True)
        for level in args.opt_levels:
            for flags in ([], ["--reachable-only"]):
                binary = root / "serve"
                subprocess.run([args.compiler.resolve(), "build", source, "--runtime", args.runtime.resolve(),
                                "--clang", args.clang, "--opt-level", level, "--no-cache", *flags,
                                "-o", binary], check=True, timeout=180)
                for name in METHODS:
                    run_case(upstream, name, [str(binary)])
                run_case(upstream, "test_handler", [str(binary)], wrong=True)
                print(f"O{level} {flags}: {len(METHODS)} original CPython client methods, native TCP server, wrong-status control")


if __name__ == "__main__":
    main()
