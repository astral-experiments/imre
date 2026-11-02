import gc
import os
import signal
import subprocess


def current():
    p = subprocess.Popen(["/bin/sh", "-c", "read x; exit 7"], stdin=subprocess.PIPE)
    try:
        class Status:
            def __bool__(self):
                raise AssertionError("truth-tested returncode")

        def override():
            value = Status()
            value.cycle = value
            p.returncode = value
            return 99

        p.poll = override
        p.send_signal(object())
        del p.poll
        gc.collect()
        assert p.returncode.cycle is p.returncode
        assert p.poll() is p.returncode and p.wait() is p.returncode

        p.returncode = None
        p.poll = lambda: 123
        p.send_signal(0)

        def failed_poll():
            raise ProcessLookupError("from poll")

        p.poll = failed_poll
        try:
            p.kill()
        except ProcessLookupError as exc:
            assert exc.args == ("from poll",)
        else:
            raise AssertionError("poll error suppressed")
    finally:
        if "poll" in p.__dict__:
            del p.poll
        p.returncode = None
        p.stdin.close()
        assert p.wait(timeout=10) == 7

    p.poll = lambda: None
    p.returncode = None
    p.send_signal(signal.SIGTERM)
    del p.poll
    assert p.poll() == 0
    p.send_signal(object())
    try:
        os.waitpid(p.pid, os.WNOHANG)
    except ChildProcessError:
        pass
    else:
        raise AssertionError("child was not reaped")
