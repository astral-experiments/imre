import errno
import io
import os
import subprocess
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
    assert subprocess.Popen is Popen and Popen.__module__ == "subprocess"
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
