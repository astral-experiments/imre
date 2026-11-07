import errno
import os
import posix
import select
import signal
import subprocess


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def pidfd():
    assert os.pidfd_open is posix.pidfd_open
    assert vars(os)["waitid"] is vars(posix)["waitid"]
    cls = os.waitid_result
    assert cls is posix.waitid_result and isinstance(cls, type)
    assert cls.__module__ == "posix" and cls.__name__ == "waitid_result"
    assert (cls.n_fields, cls.n_sequence_fields, cls.n_unnamed_fields) == (5, 5, 0)
    assert cls.__match_args__ == ("si_pid", "si_uid", "si_signo", "si_status", "si_code")
    with subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE) as child:
        assert os.waitid(Index(os.P_PID), Index(child.pid), Index(os.WEXITED | os.WNOHANG)) is None
        try:
            fd = os.pidfd_open(pid=Index(child.pid), flags=Index(0))
        except OSError as error:
            if error.errno in (errno.ENOSYS, errno.EPERM, errno.EACCES):
                return
            raise
        try:
            assert os.get_blocking(fd) and not os.get_inheritable(fd)
            poller = select.poll()
            poller.register(fd, select.POLLIN)
            assert poller.poll(0) == []
            child.stdin.close()
            assert poller.poll(5000)[0][1] & select.POLLIN
            result = posix.waitid(posix.P_PID, child.pid, os.WEXITED | os.WNOWAIT)
            assert type(result) is os.waitid_result
            assert tuple(result) == (child.pid, os.getuid(), signal.SIGCHLD, 0, os.CLD_EXITED)
            assert result.si_pid == child.pid and result.si_status == 0
            assert child.wait() == 0
            try:
                os.waitid(os.P_PID, child.pid, os.WEXITED)
            except ChildProcessError as error:
                assert error.errno == errno.ECHILD
            else:
                raise AssertionError("waitid returned a reaped child")
        finally:
            os.close(fd)


def current():
    assert os.waitpid is posix.waitpid
    assert os.WNOHANG == posix.WNOHANG
    # The current process is never its own child.
    try:
        os.waitpid(Index(os.getpid()), Index(os.WNOHANG))
    except ChildProcessError as error:
        assert error.errno == errno.ECHILD
    else:
        raise AssertionError("unexpected wait success")

    for args, kind in (((1 << 50, 0), OverflowError), ((-1, 1 << 50), OverflowError),
                       ((1.5, 0), TypeError), ((-1,), TypeError)):
        try:
            os.waitpid(*args)
        except kind:
            pass
        else:
            raise AssertionError("invalid waitpid arguments accepted")

    try:
        os.waitpid(-1, options=os.WNOHANG)
    except TypeError:
        pass
    else:
        raise AssertionError("options must be positional")
    assert os.waitstatus_to_exitcode is posix.waitstatus_to_exitcode
    assert os.WIFSTOPPED is posix.WIFSTOPPED
    assert os.WIFSTOPPED(status=Index(0x137f)) is True
    with subprocess.Popen(["/bin/sh", "-c", "exit 23"]) as child:
        pid, status = os.waitpid(child.pid, 0)
        assert pid == child.pid
        assert os.WIFSTOPPED(status=Index(status)) is False
        assert os.waitstatus_to_exitcode(status=Index(status)) == 23

    for status, kind in ((1 << 100, OverflowError), (2.5, TypeError), (-1, ValueError)):
        try:
            os.waitstatus_to_exitcode(status)
        except kind:
            pass
        else:
            raise AssertionError("invalid wait status accepted")
    return 42
