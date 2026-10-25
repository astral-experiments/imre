"""Compare selected unmodified test_socketserver methods and real TCP operations."""
import argparse
import hashlib
import json
from pathlib import Path
import resource
import select
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes

METHODS = ("test_context_manager", "test_tcpserver_bind_leak")
UP = HERE / "upstream/Lib/test/test_socketserver.py"
LEAK = b"""import os
from socketserver import StreamRequestHandler, TCPServer as RealServer

class TCPServer(RealServer):
    def server_close(self) -> None:
        os.dup(self.socket.fileno())
"""


def entry(wrong=False):
    source = b"import sys\nimport unittest\n"
    source += b"import leaking as socketserver\n" if wrong else b"import socketserver\n"
    source += b'HOST = "127.0.0.1"\nclass SocketServerTest(unittest.TestCase):\n'
    source += b"\n".join(method_bytes(UP, "SocketServerTest", name) for name in METHODS)
    source += b"\ncase = SocketServerTest()\n"
    source += b'if sys.argv[1] == "test_context_manager":\n    case.test_context_manager()\nelse:\n    case.test_tcpserver_bind_leak()\n'
    return source


def fd_limit():
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (48 if soft == resource.RLIM_INFINITY else min(48, soft), hard))


def in_fixture(case, fails=False):
    result = unittest.TestResult()
    unittest.TestSuite([case]).run(result)
    assert result.testsRun == 1 and not result.skipped, vars(result)
    if fails:
        assert not result.errors and len(result.failures) == 1, vars(result)
        assert "wrong" in result.failures[0][1] and "hello world" in result.failures[0][1], result.failures
    else:
        assert result.wasSuccessful(), (result.failures, result.errors)


class Peer:
    """The original host thread drives a bounded native handle_request loop."""
    def __init__(self, command, wrong):
        self.child = subprocess.Popen([*command, "wrong" if wrong else "echo"],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert select.select([self.child.stdout], [], [], 15)[0], "native server did not report its port"
            port = self.child.stdout.readline()
            assert port, "native server exited before announcing its port"
            self.server_address = ("127.0.0.1", int(port))
        except BaseException:
            self.close()
            raise
        self.socket = self
        self.closed_fd = None
        self.thread_error = None

    def serve_forever(self, poll_interval):
        try:
            assert select.select([self.child.stdout], [], [], 15)[0], "native server did not close"
            self.closed_fd = int(self.child.stdout.readline())
            assert self.child.wait(timeout=15) == 0, self.child.stderr.read()
        except BaseException as exc:
            self.thread_error = exc

    def shutdown(self):
        pass  # The native child was given exactly the original three requests.

    def server_close(self):
        if self.thread_error:
            raise self.thread_error
        assert self.closed_fd == -1

    def fileno(self):
        return self.closed_fd

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
        self.child.wait(timeout=15)
        self.child.stdout.close()
        self.child.stderr.close()


def observe(commands):
    from test import test_socketserver as upstream
    for method in METHODS:
        for faulty in (False, True):
            def exercise():
                p = subprocess.run([*commands["bad" if faulty else "normal"], method], capture_output=True,
                                   timeout=20, preexec_fn=fd_limit)
                if not faulty:
                    assert (p.returncode, p.stdout, p.stderr) == (0, b"", b""), p
                else:
                    marker = b"AssertionError" if method == METHODS[0] else b"Too many open files"
                    assert p.returncode != 0 and marker in p.stderr, (method, p)
            case = upstream.SocketServerTest(method)
            setattr(case, method, exercise)
            in_fixture(case)
    for wrong in (False, True):
        peer = Peer(commands["server"], wrong)
        try:
            case = upstream.SocketServerTest("test_TCPServer")
            case.make_server = lambda addr, server, handler: peer
            in_fixture(case, fails=wrong)
            if peer.thread_error:
                raise peer.thread_error
        finally:
            peer.close()


def evidence(level, ref):
    names = [(case.id(), type(case).__module__ + "." + type(case).__qualname__)
             for case in cases(load_suite(ref["module"]))]
    observed = {}
    for method in (*METHODS, "test_TCPServer"):
        host = method == "test_TCPServer"
        full = f"test.test_socketserver.SocketServerTest.{method}"
        observed[full, 1] = {
            "outcome": "pass",
            "roles": roles(body="cpython" if host else "imre", assertions="cpython" if host else "imre",
                           operation="imre", fixture="mixed"),
            "evidence": {
                "method_sha256": hashlib.sha256(method_bytes(UP, "SocketServerTest", method)).hexdigest(),
                "fixture": ("Original host test, threaded run_server and stream_examine. A native process handles three requests; no native serve_forever or shutdown is claimed."
                            if host else "Original host TestCase setup and cleanup; unchanged method runs natively under a 48-fd limit."),
                "operation": {"test_TCPServer": "Native TCPServer and StreamRequestHandler echo three lines; a wrong reply fails the original stock wire assertion.",
                              "test_context_manager": "Native context closes the listening socket; an omitted close fails the original fileno assertion.",
                              "test_tcpserver_bind_leak": "All 1024 failed binds close their listener; leaking a live duplicate exhausts the fd limit."}[method],
                "opt_level": level,
            },
        }
    return build_report(ref, level, names, observed)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--compiler", required=True, type=Path)
    ap.add_argument("--runtime", required=True, type=Path)
    ap.add_argument("--clang", default="clang")
    ap.add_argument("--output", type=Path)
    args = ap.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        ap.error("a test-enabled CPython 3.14.5 is required")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        paths = {}
        for key, data in (("normal", entry()), ("bad", entry(True)), ("server", (HERE / "serve.py").read_bytes())):
            directory = work / key
            directory.mkdir()
            if key == "bad":
                (directory / "leaking.py").write_bytes(LEAK)
            paths[key] = directory / "case.py"
            paths[key].write_bytes(data)
        observe({key: [sys.executable, "-E", str(path)] for key, path in paths.items()})
        reports = []
        for level in (0, 3):
            chain = Toolchain(args.compiler, args.runtime, Path(shutil.which(args.clang)), level)
            commands = {key: [str(chain.build(path.read_bytes(), path.parent))] for key, path in paths.items()}
            observe(commands)
            reports.append(evidence(level, ref))
            print(f"O{level}: two original native bodies, one host-driven TCP method, three negative controls")
        if args.output:
            write_reports(args.output, reports, ref)


if __name__ == "__main__":
    main()
