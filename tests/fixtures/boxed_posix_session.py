import errno
import os
import posix
import subprocess


class CurrentProcess:
    def __index__(self):
        return 0


def current():
    assert os.getsid is posix.getsid
    parent_sid = os.getsid(CurrentProcess())
    assert parent_sid == os.getsid(os.getpid())
    for create in (False, True):
        # Keep cat blocked until the parent has observed its real session.
        with subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              start_new_session=create) as child:
            try:
                sid = os.getsid(child.pid)
                assert sid == (child.pid if create else parent_sid)
                assert (sid != parent_sid) == create
            finally:
                out, err = child.communicate(b"session")
            assert (out, err, child.returncode) == (b"session", None, 0)
        try:
            os.waitpid(child.pid, os.WNOHANG)
        except ChildProcessError as error:
            assert error.errno == errno.ECHILD
        else:
            raise AssertionError("child still waitable")

    for arg, kind in ((1 << 64, OverflowError), (1.5, TypeError)):
        try:
            os.getsid(arg)
        except kind:
            pass
        else:
            raise AssertionError("invalid process ID accepted")
    try:
        os.getsid(2147483647)
    except ProcessLookupError as error:
        assert error.errno == errno.ESRCH
    else:
        raise AssertionError("nonexistent process identified")
