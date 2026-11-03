import os
import signal
import sys


def current(child_callbacks=True):
    shell = "/bin/sh"
    fd = os.open(shell, os.O_RDONLY)
    try:
        # Compare inodes so a reused descriptor number cannot appear to be inherited.
        probe = 'test "/proc/self/fd/$1" -ef /bin/sh'
        assert not os.get_inheritable(fd)
        assert os.spawnl(os.P_WAIT, shell, "sh", "-c", probe, "sh", str(fd)) == 1
        os.set_inheritable(fd, True)
        assert os.spawnl(os.P_WAIT, shell, "sh", "-c", probe, "sh", str(fd)) == 0
    finally:
        os.close(fd)

    assert os.spawnl(os.P_WAIT, shell, "sh", "-c", "kill -TERM $$") == -signal.SIGTERM
    child = os.spawnv(os.P_NOWAIT, "/dev/null/missing", ["missing"])
    assert child > 0 and child != os.getpid()
    pid, status = os.waitpid(child, 0)
    assert pid == child and os.waitstatus_to_exitcode(status) == 127

    if not child_callbacks:
        return
    parent = os.getpid()
    events = []

    class Shell:
        def __fspath__(self):
            events.append("path")
            return shell

    def audit(event, args):
        if event == "os.fork":
            events.append("fork")
        elif event == "os.exec":
            assert os.getpid() != parent and events == ["fork", "path"]
            os.putenv("IMRE_SPAWN_CHILD", str(os.getpid()))

    sys.addaudithook(audit)
    try:
        os.spawnv(os.P_WAIT, Shell(), [])
    except ValueError:
        pass
    else:
        raise AssertionError("empty argv was accepted")
    assert not events
    child = os.spawnl(os.P_NOWAIT, Shell(), "sh", "-c", 'test "$$" = "$IMRE_SPAWN_CHILD"')
    pid, status = os.waitpid(child, 0)
    assert pid == child and os.waitstatus_to_exitcode(status) == 0
    assert events == ["fork"]
