import errno
import os
import posix
import subprocess


class CurrentProcess:
    def __index__(self):
        return 0


class FirstTrue:
    def __init__(self):
        self.calls = 0

    def __bool__(self):
        self.calls += 1
        return self.calls == 1


class Program:
    def __init__(self):
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        return "session-argv0" if self.calls == 3 else "/bin/sh"


def current():
    assert os.getsid is posix.getsid
    parent_sid = os.getsid(CurrentProcess())
    assert os.getpgid is posix.getpgid and os.setpgid is posix.setpgid
    group = os.getpgid(pid=CurrentProcess())
    assert os.getpgrp is posix.getpgrp
    assert os.getpgrp() == group == os.getpgid(os.getpid())
    for args, kwargs in [((0,), {}), ((), {"pid": 0})]:
        try:
            os.getpgrp(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("getpgrp accepted an argument")
    if os.getpid() != parent_sid:
        assert os.setpgid(CurrentProcess(), group) is None
    assert parent_sid == os.getsid(os.getpid())
    for create, executable, process_group in (
        (False, None, None), (True, None, None),
        (False, "/bin/cat", None), (True, "/bin/cat", None),
        (False, "/bin/cat", 0),
    ):
        # An absolute executable override with close_fds=False is eligible for posix_spawn
        # unless a new session or group is requested. Hold cat open to observe both.
        args = ["/bin/cat"] if executable is None else ["alternate-argv0"]
        with subprocess.Popen(args, executable=executable, close_fds=executable is None,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              start_new_session=create, process_group=process_group) as child:
            try:
                assert os.getpgid(child.pid) == (child.pid if create or process_group == 0 else group)
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

    # Observe both settings on the same live child before releasing its stdin.
    with open(__file__, "rb") as marker:
        fd = marker.fileno()
        args = ["/bin/sh", "-c", 'test "/dev/fd/$1" -ef "$2" && cat', "sh", str(fd), __file__]
        with subprocess.Popen(args, process_group=0, pass_fds=(fd,), stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE) as child:
            try:
                assert os.getpgid(child.pid) == child.pid
            finally:
                out, err = child.communicate(b"kept")
            assert (out, err, child.returncode) == (b"kept", None, 0)
        assert not os.get_inheritable(fd)

    # A failed spawn eligibility check and the subsequent fork can observe different
    # values. Compare the resulting session and argv[0] with CPython on this host.
    flag = FirstTrue()
    program = Program()
    with subprocess.Popen([program, "-c", 'read word; printf "%s" "$0"'],
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          close_fds=False, start_new_session=flag) as child:
        try:
            inherited = os.getsid(child.pid) == parent_sid
        finally:
            output, _ = child.communicate(b"ready\n")
        assert child.returncode == 0
    print(flag.calls, program.calls, inherited, output)

    program = Program()
    with subprocess.Popen([program, "-c", 'test "$0" = session-argv0']) as child:
        print("default argv", child.wait(), program.calls)

    refusal = OSError("argv conversion refused")
    class RefusingProgram:
        calls = 0

        def __fspath__(self):
            self.calls += 1
            if self.calls == 2:
                raise refusal
            return "/bin/true"

    program = RefusingProgram()
    try:
        subprocess.Popen([program])
    except OSError as error:
        assert error is refusal and program.calls == 2
    else:
        raise AssertionError("Popen skipped a path conversion")

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
