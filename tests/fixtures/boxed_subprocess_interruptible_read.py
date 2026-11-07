import os
import signal
import subprocess
import tempfile


def exchange(directory, bufsize, raising, both_pipes=False):
    # The child's final write depends on the Python handler, while the parent is reading to EOF.
    gate = os.path.join(directory, str(bufsize) + ("-both" if both_pipes else ""))
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
    if both_pipes:
        command = "printf stdout\n" + command
    readfd, writefd = os.pipe()
    try:
        with subprocess.Popen(["/bin/sh", "-c", command, "sh", gate],
                              stdin=readfd, bufsize=bufsize,
                              stdout=subprocess.PIPE if both_pipes or not raising else None,
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
                        child.communicate(timeout=8 if both_pipes else None)
                    except ValueError as error:
                        assert str(error) == "pipe interrupted"
                    else:
                        raise AssertionError("handler exception did not propagate")
                    assert not stream.closed
                    if both_pipes:
                        # The selector drained both prefixes before the signal. Retry must
                        # preserve them along with the final write unblocked by the handler.
                        assert child.communicate(timeout=8) == (b"stdout", b"bcdone")
                    else:
                        assert child.communicate() == (None, b"done")
                else:
                    assert child.communicate() == (b"bcdone", None)
                assert child.wait(timeout=1) == 0 and child.returncode == 0 and seen == [signal.SIGUSR1]
            finally:
                signal.signal(signal.SIGUSR1, previous)
    finally:
        os.close(readfd)
        os.close(writefd)


def current():
    with subprocess.Popen(["/bin/printf", "abcdMORE"], stdout=subprocess.PIPE, bufsize=4) as child:
        reader = child.stdout
        assert reader.read(1) == b"a"
        raw = reader.raw
        refused = OSError("refused raw read")
        sizes = []

        def veto(window):
            sizes.append(len(window))
            raise refused

        raw.readinto = veto
        target = bytearray(b"????????")
        try:
            reader.readinto(target)
        except OSError as error:
            assert error is refused
        else:
            raise AssertionError("buffer ignored raw.readinto")
        assert sizes == [5] and target == b"bcd?????"
        assert raw.read(4) == b"MORE"

    with tempfile.TemporaryDirectory() as directory:
        exchange(directory, -1, False)
        exchange(directory, 0, True)
        exchange(directory, 0, True, both_pipes=True)
    return 42
