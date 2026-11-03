import errno
import io
import os
import subprocess
import sys
from pathlib import Path
from subprocess import Popen


def ignored_signals(restore):
    with Popen(["cat", "/proc/self/status"], stdout=subprocess.PIPE,
               restore_signals=restore) as child:
        data, _ = child.communicate()
        assert child.returncode == 0
    return next(line for line in data.split(b"\n") if line.startswith(b"SigIgn:"))


def current():
    # Check before any writes initialize the runtime's standard streams.
    if os.path.exists("/proc/self/status"):
        inherited = ignored_signals(False)
        assert ignored_signals(True) != inherited
        assert ignored_signals(False) == inherited
    report_umask = ["/bin/sh", "-c", "umask"]
    inherited_mask = int(subprocess.check_output(report_umask), 8)
    child_mask = inherited_mask ^ 0o077
    assert int(subprocess.check_output(report_umask, umask=child_mask), 8) == child_mask
    assert int(subprocess.check_output(report_umask), 8) == inherited_mask
    class InvalidMask:
        def __index__(self):
            raise RuntimeError("unusable mask")

    try:
        Popen(["/bin/true"], cwd=".", env={"x": "\x00"}, umask=InvalidMask())
    except RuntimeError as exc:
        assert str(exc) == "unusable mask"
    else:
        raise AssertionError("fork accepted an invalid umask")

    assert subprocess.Popen is Popen and Popen.__module__ == "subprocess"
    args = ["imre-argv-zero", "-c", 'printf "%s" "$0"']
    with Popen(args, executable=b"./sh", cwd="/bin", stdout=subprocess.PIPE) as child:
        assert child.args is args
        assert child.stdout.read() == b"imre-argv-zero"
        assert child.wait() == 0

    class ChangingExecutable:
        def __init__(self):
            self.calls = 0

        def __fspath__(self):
            self.calls += 1
            return "/bin/false" if self.calls == 1 else "/bin/true"

    changing = ChangingExecutable()
    with Popen(["ignored"], executable=changing, cwd=".") as child:
        print("changing executable", changing.calls, child.wait())

    with Popen("exit 47", shell=True, executable="/bin/false") as child:
        assert child.wait() == 1

    executable = Path("null/child")
    try:
        Popen(["/bin/true"], executable=executable, cwd="/dev")
    except NotADirectoryError as exc:
        assert exc.errno == errno.ENOTDIR and exc.filename is executable
    else:
        raise AssertionError("launched a program below /dev/null")

    text = subprocess.run(["/bin/sh", "-c", "printf 'one\\r\\ntwo\\r'"],
                          text=True, capture_output=True, check=True)
    assert text.stdout == "one\ntwo\n" and text.stderr == "" and text.returncode == 0
    data = b"a\x00\xff" * 65536
    # Write more than a pipe holds before reading the parent's input.
    cmd = ["/bin/sh", "-c", "dd if=/dev/zero bs=1024 count=192 >&2 2>/dev/null; cat; exit 7"]
    result = subprocess.run(cmd, input=data, capture_output=True, timeout=20)
    assert result.stdout == data
    assert result.stderr == b"\x00" * len(data)
    assert result.returncode == 7
    with subprocess.Popen(["/bin/cat"], bufsize=0, stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE) as child:
        assert type(child.stdin) is io.FileIO and type(child.stdout) is io.FileIO
        assert child.stdin.write(b"ok") == 2
        child.stdin.close()
        assert child.stdout.read(1) == b"o"
        assert child.communicate() == (b"k", None)
        assert child.stdout.closed and child.returncode == 0
    try:
        subprocess.run(["/bin/sleep", "10"], input=b"", capture_output=True, timeout=0)
    except subprocess.TimeoutExpired as exc:
        assert exc.timeout == 0
    else:
        raise AssertionError("sleep exited before its zero timeout")
    child = Popen(["/bin/true"], bufsize=0, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        os.close(child.stdout.fileno())
        try:
            child.communicate(timeout=2)
        except OSError as exc:
            assert exc.errno == errno.EBADF
        else:
            raise AssertionError("invalid pipe descriptor did not raise EBADF")
    finally:
        try:
            child.stdout.close()
        except OSError as exc:
            assert exc.errno == errno.EBADF
        child.stderr.close()
        child.wait()

    child = Popen(["/bin/sleep", "3"], stdout=subprocess.PIPE)
    try:
        try:
            with child:
                raise KeyboardInterrupt
        except KeyboardInterrupt:
            assert child.stdout.closed and child.poll() is None
            assert child._sigint_wait_secs == 0
        else:
            raise AssertionError("context suppressed KeyboardInterrupt")
    finally:
        child.kill()
        child.wait()
    for cwd in (None, "/"):
        for key in ("", "=prefix", "too=many"):
            try:
                status = subprocess.run(["/bin/true"], cwd=cwd, env={key: "value"}, close_fds=False)
            except ValueError as exc:
                assert str(exc) == "illegal environment variable name"
                print(cwd, key, "rejected")
            else:
                assert status.returncode == 0
                print(cwd, key, "accepted")
    print("subprocess FileIO verified")

    class HookError(Exception):
        pass

    endpoints = []

    class Refuse(Popen):
        def _execute_child(self, *args):
            fds = args[10:16]
            assert (self.stdin.fileno(), self.stdout.fileno(), self.stderr.fileno()) == (
                fds[1], fds[2], fds[4])
            assert len(set(fds)) == 6
            for fd in fds:
                os.fstat(fd)
            endpoints.extend(fds)
            raise HookError

    pipe = subprocess.PIPE
    try:
        Refuse(["/bin/true"], stdin=pipe, stdout=pipe, stderr=pipe)
    except HookError:
        pass
    else:
        raise AssertionError("launch ignored the overriding method")
    for fd in endpoints:
        try:
            os.fstat(fd)
        except OSError as exc:
            assert exc.errno == errno.EBADF
        else:
            raise AssertionError("constructor left an open endpoint")

    class Forward(Popen):
        def _execute_child(self, *args):
            super()._execute_child(*args)
            assert self.pid > 0

    with Forward(["/bin/cat"], stdin=pipe, stdout=pipe, stderr=pipe) as child:
        assert child.communicate(b"forwarded") == (b"forwarded", b"")
        assert child.returncode == 0

    reader, writer = os.pipe()
    try:
        with Forward(["/bin/echo", "borrowed"], stdout=writer, stderr=subprocess.STDOUT) as child:
            assert child.wait() == 0
        os.write(writer, b"open")
        assert os.read(reader, 64) == b"borrowed\nopen"
    finally:
        os.close(reader)
        os.close(writer)

    retained = []

    class AfterLaunch(Popen):
        def _execute_child(self, *args):
            assert args[-1] == -1
            super()._execute_child(*args)
            retained.append(self)
            raise HookError

    try:
        AfterLaunch(["/bin/sh", "-c", "exit 23"], stdin=pipe, stdout=pipe, stderr=pipe)
    except HookError:
        pass
    else:
        raise AssertionError("overriding exception lost after launch")
    child, = retained
    assert child.stdin.closed and child.stdout.closed and child.stderr.closed
    assert child.wait() == 23

    reader, writer = os.pipe()
    saved_stdout = sys.__stdout__
    try:
        class StdoutFD:
            def fileno(self):
                return writer

        sys.__stdout__ = StdoutFD()
        with Forward(["/bin/sh", "-c", "printf err >&2"], stderr=subprocess.STDOUT) as child:
            assert child.wait() == 0
        os.write(writer, b"end")
        assert os.read(reader, 6) == b"errend"
    finally:
        sys.__stdout__ = saved_stdout
        os.close(reader)
        os.close(writer)
