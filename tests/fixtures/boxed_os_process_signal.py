import errno
import os
import posix
import signal
import subprocess
import sys


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def current():
    assert os.getppid is posix.getppid
    assert os.kill is posix.kill
    assert os.getppid() >= 0 and os.getppid() != os.getpid()
    observed = []
    reject = []

    def audit(event, arguments):
        if event == "os.kill":
            observed.append(arguments)
            if reject:
                raise RuntimeError("vetoed")

    sys.addaudithook(audit)
    pid = os.getpid()
    assert os.kill(Index(pid), Index(0)) is None
    assert observed[-1] == (pid, 0)
    if sys.maxsize > 0xffffffff:
        assert os.kill(pid, 1 << 32) is None
        assert observed[-1] == (pid, 1 << 32)
    with subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE) as child:
        reject.append(True)
        try:
            os.kill(child.pid, signal.SIGTERM)
        except RuntimeError as error:
            assert str(error) == "vetoed"
        else:
            raise AssertionError("audit hook did not veto kill")
        reject.clear()
        assert child.poll() is None
        os.kill(child.pid, signal.SIGTERM)
        assert child.wait() == -signal.SIGTERM

    for args, kind in (((1 << 80, 0), OverflowError), ((pid, 1 << 80), OverflowError),
                       ((1.5, 0), TypeError), ((pid, "0"), TypeError), ((pid,), TypeError)):
        try:
            os.kill(*args)
        except kind:
            pass
        else:
            raise AssertionError("invalid kill arguments accepted")
    try:
        os.kill(pid, 128)
    except OSError as error:
        assert error.errno == errno.EINVAL
    else:
        raise AssertionError("invalid signal accepted")
    try:
        os.kill(0x7fffffff, 0)
    except ProcessLookupError as error:
        assert error.errno == errno.ESRCH
    else:
        raise AssertionError("expected a missing process")
    try:
        os.kill(pid=pid, signal=0)
    except TypeError:
        pass
    else:
        raise AssertionError("expected positional-only arguments")
    return 42
