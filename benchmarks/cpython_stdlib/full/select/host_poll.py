"""Run original test_poll1 in CPython, forwarding each poll wait to the native child."""
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
    parser.add_argument("--drop-write", action="store_true")
    args = parser.parse_args()
    from test import test_poll

    root = Path(__file__).resolve().parent
    assert Path(test_poll.__file__).read_bytes() == (root / "upstream/Lib/test/test_poll.py").read_bytes()

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

        def poll(self):
            parameters = [str(n) for item in self.interest.items() for n in item]
            result = subprocess.run([args.binary, "poll", "-1", *parameters],
                                    pass_fds=tuple(self.interest), check=True,
                                    capture_output=True, text=True, timeout=5)
            words = list(map(int, result.stdout.splitlines()))
            ready = list(zip(words[::2], words[1::2], strict=True))
            return [(fd, mask) for fd, mask in ready
                    if not args.drop_write or not (mask & select.POLLOUT)]

    api = SimpleNamespace(poll=Bridge, POLLIN=select.POLLIN, POLLOUT=select.POLLOUT)
    with patch.object(test_poll, "select", api):
        result = unittest.TestResult()
        test_poll.PollTests("test_poll1").run(result)
    if args.drop_write:
        assert len(result.errors) == 1 and "no pipes ready for writing" in result.errors[0][1], result.errors
    else:
        assert result.wasSuccessful() and not result.skipped, (result.errors, result.failures, result.skipped)
    print("host-driven original test_poll1 passed" if not args.drop_write else "drop-write control detected")


if __name__ == "__main__":
    main()
