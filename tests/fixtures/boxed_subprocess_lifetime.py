import os
import select
import subprocess
import sys
import warnings


def current():
    child = subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE)
    writer = child.stdin
    pid = child.pid
    ident = id(child)
    try:
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always", ResourceWarning)
            child = None
            assert len(caught) == 1
            warning = caught[0]
            assert warning.category is ResourceWarning
            assert str(warning.message) == "subprocess %s is still running" % pid
            owner = warning.source
            assert id(owner) == ident and owner in subprocess._active
            writer.close()
            assert owner.wait() == 0
            subprocess._cleanup()
            assert owner not in subprocess._active
    finally:
        writer.close()
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", ResourceWarning)
        child = subprocess.Popen(["/bin/sh", "-c", "exit 7"])
        assert child.wait() == 7
        child = None
        assert not caught

    with subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE) as child:
        with warnings.catch_warnings():
            warnings.simplefilter("error", ResourceWarning)
            try:
                child.__del__()
            except ResourceWarning:
                pass
            else:
                raise AssertionError("expected ResourceWarning")
            assert child.returncode is None and child not in subprocess._active

        def observe(message, category, filename, lineno, file=None, line=None):
            child.stdin.close()
            assert child.wait() == 0

        with warnings.catch_warnings():
            warnings.simplefilter("always", ResourceWarning)
            warnings.showwarning = observe
            child.__del__()
        assert child.returncode == 0 and child not in subprocess._active

    # A hook which saves just the message leaves no owner of the child's stdin.
    messages = []

    def report(args):
        messages.append(str(args.exc_value))

    child = subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    reader = child.stdout
    pid = child.pid
    hook = sys.unraisablehook
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("error", ResourceWarning)
            sys.unraisablehook = report
            child = None
        assert messages.count("subprocess %s is still running" % pid) == 1
        assert select.select([reader], [], [], 5)[0] and reader.read(1) == b""
        try:
            assert os.waitpid(pid, 0) == (pid, 0)
        except ChildProcessError:
            pass
    finally:
        sys.unraisablehook = hook
        reader.close()
        try:
            if os.waitpid(pid, os.WNOHANG)[0] == 0:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
        except ChildProcessError:
            pass
