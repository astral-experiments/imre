import os
import select
import signal
import subprocess


def current():
    # The callback sees redirected stdout and cwd in the same process that execs sh.
    fd = os.dup(1)
    try:
        def redirect():
            os.dup2(1, fd)
            os.write(1, f"callback {os.getpid()} {os.getcwd()}\n".encode())

        report = ["/bin/sh", "-c",
                  'if test -e "/proc/self/fd/$1"; then state=open; else state=closed; fi; '
                  'printf "target %d %s\\n" "$$" "$state"; exit 23', "sh", str(fd)]
        for close_fds, state in ((False, "open"), (True, "closed")):
            proc = subprocess.Popen(report, stdout=subprocess.PIPE, cwd="/",
                                    close_fds=close_fds, preexec_fn=redirect)
            output, _ = proc.communicate()
            assert proc.returncode == 23
            assert output == f"callback {proc.pid} /\ntarget {proc.pid} {state}\n".encode()
        assert not os.get_inheritable(fd)

        # pass_fds makes fd inheritable before the callback. Closing it there must stick.
        def close_kept():
            assert os.get_inheritable(fd)
            os.close(fd)

        proc = subprocess.Popen(report, stdout=subprocess.PIPE, pass_fds=(fd,),
                                preexec_fn=close_kept)
        output, _ = proc.communicate()
        assert proc.returncode == 23 and output == f"target {proc.pid} closed\n".encode()
        assert not os.get_inheritable(fd)
    finally:
        os.close(fd)

    reader, writer = os.pipe()
    try:
        def fail():
            os.write(1, b"callback raised\n")
            raise LookupError("child-only exception")

        before = set(os.listdir("/proc/self/fd"))
        try:
            subprocess.Popen(["/bin/sh", "-c", "echo unexpected-target"],
                             stdin=subprocess.PIPE, stdout=writer, stderr=subprocess.PIPE,
                             preexec_fn=fail)
        except subprocess.SubprocessError as exc:
            assert str(exc) == "Exception occurred in preexec_fn."
        else:
            raise AssertionError("preexec_fn exception did not reach the parent")
        assert set(os.listdir("/proc/self/fd")) == before
        assert os.read(reader, 1024) == b"callback raised\n"
    finally:
        os.close(reader)
        os.close(writer)

    # A signal during startup may interrupt the constructor while its child is alive.
    retained = []

    class Retained(subprocess.Popen):
        def __init__(self, *args, **kwargs):
            retained.append(self)
            super().__init__(*args, **kwargs)

    reader, writer = os.pipe()
    owner = os.getpid()
    interrupted = RuntimeError("interrupted during child setup")

    def hold_child():
        os.close(writer)
        os.kill(owner, signal.SIGUSR1)
        if not select.select([reader], [], [], 5)[0] or os.read(reader, 1) != b"x":
            os._exit(90)

    def interrupt(signum, frame):
        assert retained[0].pid > 0
        assert retained[0].poll() is None
        raise interrupted

    old_handler = signal.signal(signal.SIGUSR1, interrupt)
    try:
        try:
            Retained(["/bin/sh", "-c", "exit 23"], preexec_fn=hold_child)
        except RuntimeError as exc:
            assert exc is interrupted
        else:
            raise AssertionError("handler did not interrupt the constructor")
        os.write(writer, b"x")
        assert retained[0].wait(timeout=5) == 23
        try:
            os.waitpid(retained[0].pid, os.WNOHANG)
        except ChildProcessError:
            pass
        else:
            raise AssertionError("Popen.wait did not reap the child")
    finally:
        signal.signal(signal.SIGUSR1, old_handler)
        os.close(writer)
        os.close(reader)
