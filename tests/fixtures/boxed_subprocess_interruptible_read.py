import os
import signal
import subprocess
import tempfile


def exchange(directory, bufsize, raising):
    # The child's final write depends on the Python handler, while the parent is reading to EOF.
    gate = os.path.join(directory, str(bufsize))
    command = """
        printf abc
        read start || exit 41
        sleep 0.1
        kill -USR1 "$PPID" || exit 42
        i=0
        while [ ! -f "$1" ] && [ "$i" -lt 100 ]; do
            sleep 0.05
            i=$((i + 1))
        done
        [ -f "$1" ] || exit 43
        printf done
    """
    if raising:
        command = "exec >&2\n" + command
    readfd, writefd = os.pipe()
    try:
        with subprocess.Popen(["/bin/sh", "-c", command, "sh", gate],
                              stdin=readfd, bufsize=bufsize,
                              stdout=None if raising else subprocess.PIPE,
                              stderr=subprocess.PIPE if raising else None) as child:
            stream = child.stderr if raising else child.stdout
            seen = []

            def handler(signum, frame):
                assert child.poll() is None
                if bufsize != 0:
                    try:
                        stream.read(1)
                    except RuntimeError as error:
                        assert "reentrant call" in str(error)
                    else:
                        raise AssertionError("nested buffered read succeeded")
                with open(gate, "wb"):
                    pass
                seen.append(signum)
                if raising:
                    raise ValueError("pipe interrupted")

            previous = signal.signal(signal.SIGUSR1, handler)
            try:
                assert stream.read(1) == b"a"
                os.write(writefd, b"start\n")
                if raising:
                    try:
                        child.communicate()
                    except ValueError as error:
                        assert str(error) == "pipe interrupted"
                    else:
                        raise AssertionError("handler exception did not propagate")
                    assert not stream.closed
                    assert child.communicate() == (None, b"done")
                else:
                    assert child.communicate() == (b"bcdone", None)
                assert child.returncode == 0 and seen == [signal.SIGUSR1]
            finally:
                signal.signal(signal.SIGUSR1, previous)
    finally:
        os.close(readfd)
        os.close(writefd)


def current():
    with tempfile.TemporaryDirectory() as directory:
        exchange(directory, -1, False)
        exchange(directory, 0, True)
    return 42
