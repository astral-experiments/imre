"""Host original CPython unittests, forwarding fd operations to the native child."""
import argparse
from pathlib import Path
import select
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--controls", action="store_true")
    args = parser.parse_args()
    from test import test_poll

    root = Path(__file__).resolve().parent
    assert Path(test_poll.__file__).read_bytes() == (root / "upstream/Lib/test/test_poll.py").read_bytes()

    completed_waits = []

    class Bridge:
        def __init__(self):
            self.interest = {}

        def register(self, fd, events=select.POLLIN | select.POLLPRI | select.POLLOUT):
            self.interest[fd] = events

        def modify(self, fd, events):
            assert fd in self.interest
            self.interest[fd] = events

        def unregister(self, fd):
            del self.interest[fd]

        def poll(self, timeout=None):
            parameters = [str(n) for item in self.interest.items() for n in item]
            if args.controls and method == "test_poll_blocks_with_negative_ms":
                timeout = 0
            token = f"float:{timeout!r}" if isinstance(timeout, float) else str(timeout)
            result = subprocess.run([args.binary, "poll", token, *parameters],
                                    pass_fds=tuple(self.interest), check=True,
                                    capture_output=True, text=True, timeout=5)
            words = list(map(int, result.stdout.splitlines()))
            ready = list(zip(words[::2], words[1::2], strict=True))
            if method == 'test_poll_blocks_with_negative_ms' and not args.controls:
                assert ready == [(fd, select.POLLIN) for fd in self.interest], ready
                completed_waits.append(timeout)
            return [(fd, mask) for fd, mask in ready
                    if not (args.controls and method == "test_poll1" and mask & select.POLLOUT)]

    class CLimits:
        def invoke(self, operation, value):
            result = subprocess.run([args.binary, 'limit', operation, str(value)], capture_output=True, text=True, timeout=5)
            if result.returncode:
                assert 'OverflowError:' in result.stderr, result.stderr
                if not args.controls:
                    raise OverflowError(result.stderr.strip())

        def register(self, fd, events=None):
            if events is None:
                assert fd == 1  # The child starts with this registration.
            else:
                assert fd == 0
                self.invoke('register', events)

        def modify(self, fd, events):
            assert fd == 1
            self.invoke('modify', events)

        def poll(self, timeout):
            self.invoke('poll', timeout)

    observed = []
    def select_pipe(readers, writers, exceptional, timeout):
        stream, = readers
        assert not writers and not exceptional
        fd = stream.fileno()
        result = subprocess.run([args.binary, 'select', str(fd), str(timeout)], pass_fds=(fd,), check=True,
                                capture_output=True, text=True, timeout=5)
        ready = list(map(int, result.stdout.splitlines()))
        assert ready in ([], [fd]), ready
        if ready:
            observed.append(fd)
        return ([stream] if ready else [], [stream] if args.controls else [], [])

    from test import test_select
    assert Path(test_select.__file__).read_bytes() == (root / 'upstream/Lib/test/test_select.py').read_bytes()
    for module, owner, method, api, failure in (
        (test_poll, test_poll.PollTests, 'test_poll1', SimpleNamespace(
            poll=Bridge, POLLIN=select.POLLIN, POLLOUT=select.POLLOUT), 'no pipes ready for writing'),
        (test_poll, test_poll.PollTests, 'test_poll_blocks_with_negative_ms', SimpleNamespace(
            poll=Bridge, POLLIN=select.POLLIN), 'is not true'),
        (test_poll, test_poll.PollTests, 'test_poll_c_limits', SimpleNamespace(poll=CLimits), 'OverflowError not raised'),
        (test_select, test_select.SelectTestCase, 'test_select', SimpleNamespace(select=select_pipe), 'Lists differ'),
    ):
        with patch.object(module, 'select', api):
            result = unittest.TestResult()
            owner(method).run(result)
        if args.controls:
            problems = result.errors + result.failures
            assert len(problems) == 1 and failure in problems[0][1], (method, problems)
        else:
            assert result.wasSuccessful() and not result.skipped, (method, result.errors, result.failures, result.skipped)
            if method == 'test_select':
                assert observed, 'the original method never observed a readable pipe'
            elif method == 'test_poll_blocks_with_negative_ms':
                # A failed worker thread does not itself fail unittest. Require
                # every native wait to finish successfully with readable data.
                assert completed_waits == [None, -1000, -1, -1.0, -0.1, -1e-100], completed_waits
        print(f'host-driven original {method}: ' + ('control detected' if args.controls else 'passed'))


if __name__ == "__main__":
    main()
