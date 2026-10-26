"""Record the arguments reaching the native parent's installed waitpid target."""

from unittest import mock

import fixture_waitpid


class Waitpid:
    child: int
    calls: mock.Mock

    def __init__(self, child: int) -> None:
        self.child = child
        self.calls = mock.Mock(name="waitpid")

    def observe(self, options: int) -> int:
        # The fixture dispatches this callback only for the installed child.
        target = self.calls
        target(self.child, options)
        target.assert_called_once_with(self.child, options)
        return target.call_count


def install(pid: int) -> None:
    target = Waitpid(pid)
    target.calls.assert_not_called()
    fixture_waitpid.enter_mock(pid, target.observe)
