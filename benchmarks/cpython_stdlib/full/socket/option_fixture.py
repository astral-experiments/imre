"""Run the upstream option tests on the host against one real socket in a child."""
from contextlib import ExitStack
import select
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch

OPTIONS = ("testGetSockOpt", "testSetSockOpt")
TIMEOUT = 15


class OptionSocket:
    def __init__(self, command, family, kind, stack, fault, events):
        self.process = subprocess.Popen([*command, "options", str(int(family)), str(int(kind))],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.fault = fault
        self.events = events
        stack.callback(self.abort)
        assert self.reply() == b"open"
        events.append("open")
        if fault == "testGetSockOpt":
            from socket import SOL_SOCKET, SO_REUSEADDR
            self.setsockopt(SOL_SOCKET, SO_REUSEADDR, 1)

    def abort(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.communicate(timeout=TIMEOUT)

    def reply(self):
        assert select.select([self.process.stdout], [], [], TIMEOUT)[0], "socket option operation timed out"
        response = self.process.stdout.readline()
        assert response, self.process.communicate(timeout=TIMEOUT)
        return response.rstrip(b"\n")

    def request(self, *fields):
        self.process.stdin.write(b"".join(str(v).encode() + b"\n" for v in fields))
        self.process.stdin.flush()

    def getsockopt(self, level, option):
        self.events.append("get")
        self.request("get", level, option)
        return int(self.reply())

    def setsockopt(self, level, option, value):
        self.events.append("set")
        self.request("set", level, option, 0 if self.fault == "testSetSockOpt" else value)
        assert self.reply() == b"ok"

    def close(self):
        self.request("close")
        out, err = self.process.communicate(timeout=TIMEOUT)
        assert (self.process.returncode, out, err) == (0, b"", b""), (out, err)
        self.events.append("close")


def observe_options(command):
    from test import test_socket as upstream
    for method in OPTIONS:
        for fault in (None, method):
            events = []
            with ExitStack() as stack:
                def open_socket(family, kind):
                    return OptionSocket(command, family, kind, stack, fault, events)
                namespace = SimpleNamespace(socket=open_socket, **{
                    name: getattr(upstream.socket, name)
                    for name in ("AF_INET", "SOCK_STREAM", "SOL_SOCKET", "SO_REUSEADDR")})
                with patch.object(upstream, "socket", namespace):
                    result = unittest.TestResult()
                    unittest.TestSuite([upstream.GeneralModuleTests(method)]).run(result)
                assert result.testsRun == 1 and not (result.errors or result.skipped), vars(result)
                assert events[0] == "open" and events[-2:] == ["get", "close"], events
                if fault:
                    assert len(result.failures) == 1 and "AssertionError" in result.failures[0][1], result.failures
                else:
                    assert result.wasSuccessful(), result.failures
