"""Run selected original test_socket methods and their companion senders over TCP."""
import argparse
import ast
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent.parent)]
from option_fixture import OPTIONS, observe_options
from socket_evidence import report
from suite_evidence import write_reports
from suite_inventory import verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes

UP = HERE / "upstream/Lib/test/test_socket.py"
TCP = ("testRecv", "testOverFlowRecv", "testSendAll")
BACKLOG = "test_listen_backlog"
DESCRIPTORS = ("test_default_inheritable", "test_set_inheritable", "test_dup")
TIMEOUT = 15
NATIVE_BYTES_EQUALITY = b'''    def assertEqual(self, actual: bytes, expected: bytes) -> None:
        if actual != expected:
            raise AssertionError("received bytes differ")
'''
NATIVE_BOOL_EQUALITY = b'''    def assertEqual(self, actual: bool, expected: bool) -> None:
        self.assertTrue(actual == expected)
'''


def source():
    bodies = {name: method_bytes(UP, "BasicTCPTest", name) for method in TCP
              for name in (method, "_" + method)}
    tcp_methods = b"\n".join(bodies.values())
    bodies[BACKLOG] = method_bytes(UP, "GeneralModuleTests", BACKLOG)
    for name in DESCRIPTORS:
        bodies[name] = method_bytes(UP, "InheritanceTest", name)
    # Use the original MSG expression and socket_helper's actual HOST constant.
    from test.test_socket import HOST
    data = UP.read_bytes()
    msg, = [node for node in ast.parse(data).body if isinstance(node, ast.Assign)
            and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "MSG"]
    globals_ = f"HOST = {HOST!r}\n".encode() + data.splitlines(keepends=True)[msg.lineno - 1]
    template = (HERE / "tcp_fixture.py.in").read_bytes()
    return (template.replace(b"# UPSTREAM_GLOBALS", globals_)
            .replace(b"# UPSTREAM_TCP_METHODS", tcp_methods)
            .replace(b"# UPSTREAM_INHERITANCE", b"\n".join(bodies[n] for n in DESCRIPTORS))
            .replace(b"# UPSTREAM_BACKLOG", bodies[BACKLOG])), bodies


def observe(command):
    from test import test_socket as upstream

    def run(*args, fd=None, wrong=False):
        p = subprocess.run([*command, *args], pass_fds=() if fd is None else (fd,),
                           timeout=TIMEOUT, capture_output=True)
        if wrong:
            assert p.returncode != 0 and b"AssertionError" in p.stderr, (args, p)
        else:
            assert (p.returncode, p.stdout, p.stderr) == (0, b"", b""), (args, p)

    def in_fixture(case):
        result = unittest.TestResult()
        unittest.TestSuite([case]).run(result)
        assert result.testsRun == 1 and result.wasSuccessful() and not result.skipped, vars(result)

    backlog = upstream.GeneralModuleTests(BACKLOG)
    setattr(backlog, BACKLOG, lambda: run("backlog"))
    in_fixture(backlog)
    for name in DESCRIPTORS:
        case = upstream.InheritanceTest(name)
        setattr(case, name, lambda name=name: run("descriptors", name))
        in_fixture(case)

    run("wrong-cloexec", wrong=True)

    def run_tcp(owner, method, wrong):
        case = owner(method)
        entered = []

        def sender():
            fd = case.serv_conn.fileno()
            run("wrong" if wrong else "sender", method, str(fd), fd=fd)
            entered.append("sender")

        def receiver():
            fd = case.cli_conn.fileno()
            run("receiver", method, str(fd), fd=fd, wrong=wrong)
            entered.append("receiver")

        # ThreadableTest looks up the companion before starting its thread.
        setattr(case, "_" + method, sender)
        setattr(case, method, receiver)
        in_fixture(case)
        assert sorted(entered) == ["receiver", "sender"], (case.id(), entered)

    for owner in (upstream.BasicTCPTest, upstream.BasicTCPTest2):
        for method in TCP:
            for wrong in (False, True):
                run_tcp(owner, method, wrong)
    observe_options(command)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("run with test-enabled CPython 3.14.5")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    entry, bodies = source()
    for method in OPTIONS:
        bodies[method] = method_bytes(UP, "GeneralModuleTests", method)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if (reference["platform"], reference["cpython_commit"]) != (sys.platform, manifest["commit"]):
        parser.error("reference evidence requires the same platform and source revision")
    reports = []
    clang = Path(shutil.which(args.clang) or args.clang)
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        script = work / "oracle.py"
        # Stock CPython uses its own TestCase.assertEqual for bytes and booleans.
        script.write_bytes(entry.replace(b"# NATIVE_BYTES_EQUALITY", b"").replace(b"# NATIVE_BOOL_EQUALITY", b""))
        native = (entry.replace(b"# NATIVE_BYTES_EQUALITY", NATIVE_BYTES_EQUALITY)
                  .replace(b"# NATIVE_BOOL_EQUALITY", NATIVE_BOOL_EQUALITY))
        observe([sys.executable, "-E", script])
        print("CPython: ten original bodies, two host-driven options; nine negative controls rejected", flush=True)
        for level in (0, 3):
            binary = Toolchain(args.compiler, args.runtime, clang, level).build(native, work)
            observe([binary])
            reports.append(report(reference, UP, bodies, TCP, BACKLOG, OPTIONS, DESCRIPTORS, level))
            print(f"O{level}: ten original bodies, two host-driven options; nine negative controls rejected", flush=True)

    if args.output:
        write_reports(args.output, reports, reference)


if __name__ == "__main__":
    main()
