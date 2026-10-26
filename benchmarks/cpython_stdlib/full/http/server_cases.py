"""Run the original CPython TCP client methods against a separately compiled server."""
import ast
from contextlib import contextmanager
import hashlib
import http.client
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify import HERE, manifests, options, toolchains
from suite_evidence import roles
from suite_native import method_bytes
from suite_inventory import verify_interpreter_tests

UPSTREAM = HERE / "upstream/Lib/test/test_httpservers.py"
METHODS = (
    "test_command", "test_request_line_trimming", "test_version_bogus",
    "test_version_digits", "test_version_signs_and_underscores",
    "test_major_version_number_too_long", "test_minor_version_number_too_long",
    "test_version_none_get", "test_version_none", "test_version_invalid",
    "test_send_blank", "test_header_close", "test_header_keep_alive", "test_handler",
    "test_return_header_keep_alive", "test_internal_key_error", "test_return_custom_status",
    "test_return_explain_error", "test_latin1_header", "test_error_content_length",
    "test_send_error", "test_head_via_send_error",
)


LEGACY_METHODS = ("test_simple_get", "test_invalid_request", "test_single_request")
DATE_CLASS = "BaseHTTPRequestHandlerTestCase"
DATE_METHOD = "test_date_time_string"


def original_tests():
    for suite, manifest in manifests():
        if suite == "httpservers":
            verify_interpreter_tests(manifest)
    from test import test_httpservers
    return test_httpservers


def program():
    source = UPSTREAM.read_text()
    tree = ast.parse(source)
    cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "BaseHTTPServerTestCase")
    handler = next(n for n in cls.body if isinstance(n, ast.ClassDef) and n.name == "request_handler")
    lines = source.splitlines(keepends=True)
    methods = [textwrap.indent(textwrap.dedent("".join(lines[n.lineno - 1:n.end_lineno])), "    ")
               for n in handler.body if isinstance(n, ast.FunctionDef) and n.name.startswith("do_")]
    legacy = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "HTTP09ServerTestCase")
    legacy_handler = next(n for n in legacy.body if isinstance(n, ast.ClassDef) and n.name == "request_handler")
    get = next(n for n in legacy_handler.body if isinstance(n, ast.FunctionDef) and n.name == "do_GET")
    legacy_get = textwrap.indent(textwrap.dedent("".join(lines[get.lineno - 1:get.end_lineno])), "    ")
    date_case = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "BaseHTTPRequestHandlerTestCase")
    date = next(n for n in date_case.body if isinstance(n, ast.FunctionDef) and n.name == "test_date_time_string")
    date_method = "".join(lines[date.lineno - 1:date.end_lineno])
    return ("""import sys
import time
import unittest
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer


class DateCase(unittest.TestCase):
    handler: BaseHTTPRequestHandler

    def __init__(self, handler: BaseHTTPRequestHandler) -> None:
        self.handler = handler
        super().__init__()

""" + date_method + """


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    default_request_version = "HTTP/1.1"
""" + "\n".join(methods) + """

    def do_DATE(self) -> None:
        try:
            DateCase(self).test_date_time_string()
        except AssertionError:
            self.send_error(500, "date assertion failed")
            return
        self.send_response(204)
        self.send_header("Connection", "close")
        self.end_headers()


class Mutant(Handler):
    def date_time_string(self, timestamp: float | None = None) -> str:
        return "Thu, 01 Jan 1970 00:00:00 GMT"

    def do_TEST(self) -> None:
        self.send_response(200)
        self.send_header("Connection", "close")
        self.end_headers()


class LegacyHandler(BaseHTTPRequestHandler):
""" + legacy_get + """


if sys.argv[1] == "legacy":
    server = HTTPServer(("127.0.0.1", 0), LegacyHandler)
elif sys.argv[1] == "wrong":
    server = HTTPServer(("127.0.0.1", 0), Mutant)
else:
    server = HTTPServer(("127.0.0.1", 0), Handler)
with server:
    print(server.server_address[1])
    sys.stdout.flush()
    for request in range(int(sys.argv[2])):
        server.handle_request()
""")


@contextmanager
def listener(command, wrong=False, requests=1, *, legacy=False):
    with tempfile.TemporaryFile() as errors:
        def diagnostic(message):
            errors.seek(0)
            return f"{command[0]} {message}\n{errors.read().decode(errors='replace')}".rstrip()

        variant = "legacy" if legacy else "wrong" if wrong else "normal"
        with subprocess.Popen([*command, variant, str(requests)], stdout=subprocess.PIPE, stderr=errors) as child:
            try:
                # HTTPServer.server_bind performs a reverse lookup before the
                # port can be published, even for a loopback listener.
                if not select.select([child.stdout], [], [], 60)[0]:
                    child.kill()
                    child.wait()
                    raise TimeoutError(diagnostic("did not publish its TCP port within 60 seconds"))
                line = child.stdout.readline()
                if not line:
                    status = child.wait(timeout=8)
                    raise AssertionError(diagnostic(f"exited with status {status} before publishing a TCP port"))
                yield int(line)
                if status := child.wait(timeout=8):
                    raise AssertionError(diagnostic(f"exited with status {status} after publishing its port"))
            finally:
                if child.poll() is None:
                    child.kill()


def check_result(case, wrong=False):
    result = unittest.TestResult()
    case.run(result)
    if result.errors or result.skipped or len(result.failures) != int(wrong):
        raise AssertionError((case.id(), result.errors, result.failures, result.skipped))
    if wrong and "200 !=" not in result.failures[0][1]:
        raise AssertionError(result.failures)


def run_case(tests, name, command, wrong=False):
    requests = 5 if name in ("test_send_error", "test_head_via_send_error") else 1
    with listener(command, wrong, requests, legacy=name in LEGACY_METHODS) as port:
        if name in LEGACY_METHODS:
            class Wire(tests.HTTP09ServerTestCase):
                def setUp(self):
                    self.sock = self.enterContext(socket.create_connection(("127.0.0.1", port), timeout=5))

                def tearDown(self):
                    self.sock.close()
        else:
            class Wire(tests.BaseHTTPServerTestCase):
                def setUp(self):
                    self.HOST, self.PORT = "127.0.0.1", port
                    self.con = http.client.HTTPConnection(self.HOST, port, timeout=5)
                    self.con.connect()

                def tearDown(self):
                    self.con.close()
        check_result(Wire(name), wrong)


def date_case(command, wrong=False):
    with listener(command, wrong) as port:
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            connection.request("DATE", "/")
            response = connection.getresponse()
            assert response.status == (500 if wrong else 204), (response.status, response.reason)
            if wrong:
                assert response.reason == "date assertion failed", response.reason
            response.read()
        finally:
            connection.close()


def public_contracts(command):
    # Compare the child's bound name with an independent CPython lookup.
    command = [*command, socket.getfqdn("127.0.0.1")]
    with listener(command) as port:
        with socket.create_connection(("127.0.0.1", port), timeout=5) as conn:
            conn.sendall(b"GET /old\r\n")
            with conn.makefile("rb") as stream:
                assert stream.read() == b"lower=0.9\n"
    with listener(command) as port:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.connect()
            first_socket = conn.sock
            for connection in ("keep-alive", "close"):
                conn.putrequest("CHECK", "/new")
                conn.putheader("Connection", connection)
                conn.putheader("X-Value", "one")
                conn.putheader("x-value", "two")
                conn.endheaders()
                assert conn.sock is first_socket
                response = conn.getresponse()
                assert response.status == 200, response.reason
                assert response.getheader("Connection") == connection
                assert response.getheader("Server").startswith("Overridden/10 ")
                assert response.read() == b"X-Value=one\nx-value=two\n"
        finally:
            conn.close()


def run_methods(tests, command):
    for name in (*METHODS, *LEGACY_METHODS):
        run_case(tests, name, command)
    run_case(tests, "test_handler", command, wrong=True)
    for wrong in (False, True):
        date_case(command, wrong)


def reference(work):
    source = work / "server_reference.py"
    source.write_text(program())
    run_methods(original_tests(), [sys.executable, "-B", source])
    public_contracts([sys.executable, "-B", HERE / "server_contract.py"])


def measure(tool, work):
    body = program().encode()
    tests = original_tests()
    run_methods(tests, [tool.build(body, work)])
    contract = (HERE / "server_contract.py").read_bytes()
    public_contracts([tool.build(contract, work)])
    return {"source_sha256": hashlib.sha256(body).hexdigest(),
            "contract_sha256": hashlib.sha256(contract).hexdigest(),
            "host_cases": [*(f"{tests.BaseHTTPServerTestCase.__name__}.{m}" for m in METHODS),
                           *(f"{tests.HTTP09ServerTestCase.__name__}.{m}" for m in LEGACY_METHODS)],
            "native_case": f"{DATE_CLASS}.{DATE_METHOD}",
            "controls": ["wrong 200 in do_TEST", "incorrect date in the native original assertion"]}


def observation(case, ordinary, reachable):
    if ordinary != reachable:
        raise ValueError("ordinary and reachable HTTP server coverage differs")
    cls, method = type(case).__name__, case._testMethodName
    name = f"{cls}.{method}"
    native = name == reachable["native_case"]
    if not native and name not in reachable["host_cases"]:
        return None
    body = "imre" if native else "cpython"
    return {"outcome": "pass", "roles": roles(body=body, assertions=body, operation="imre", fixture="mixed"),
            "evidence": {"method_sha256": hashlib.sha256(method_bytes(UPSTREAM, cls, method)).hexdigest(),
                         "fixture": ("A TCP client triggers an Imre unittest assertion on a live handler." if native
                                     else "Original CPython client method connects to an independently compiled listener."),
                         "operation": ("Native HTTP-date formatting and reference timestamp comparison." if native
                                       else "Native socketserver accept, HTTP parse, handler dispatch and response."),
                         "artifact": "measurements-linux.json"}}


def main():
    args = options(__doc__)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        reference(work)
        for tool in toolchains(args):
            row = measure(tool, work)
            print(f"O{tool.level} reachable={tool.reachable_only}: {len(row['host_cases'])} original client methods, "
                  f"{row['native_case']} compiled, both negative controls passed", flush=True)


if __name__ == "__main__":
    main()
