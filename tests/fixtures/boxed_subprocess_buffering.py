import gc
import io
import os
import subprocess
import warnings


def current():
    pipe = subprocess.PIPE
    for size in (-1, None, 0, 4):
        with subprocess.Popen(["/bin/cat"], bufsize=size, stdin=pipe,
                              stdout=pipe, stderr=pipe) as child:
            writer = child.stdin
            reader = child.stdout
            if size == 0:
                assert type(writer) is io.FileIO and type(reader) is io.FileIO
            else:
                assert isinstance(writer, io.BufferedIOBase) and isinstance(reader, io.BufferedIOBase)
                assert type(writer).__name__ == "BufferedWriter"
                assert type(reader).__name__ == "BufferedReader"
                assert type(writer.raw) is io.FileIO and type(reader.raw) is io.FileIO
                assert writer.raw.fileno() == writer.fileno()
            assert writer.write(b"abc") == 3
            assert child.communicate(b"def") == (b"abcdef", b"")
            assert child.returncode == 0
            assert writer.closed and reader.closed and child.stderr.closed
            if size != 0:
                assert writer.raw.closed and reader.raw.closed

    for value, expected in ((None, -1), (1, 1)):
        with subprocess.Popen(["/bin/true"], stdin=pipe, stdout=pipe, stderr=pipe,
                              pipesize=value) as child:
            assert child.pipesize == expected and child.wait() == 0
        assert child.stdin.closed and child.stdout.closed and child.stderr.closed

    try:
        subprocess.Popen(["/bin/true"], pipesize="invalid")
    except TypeError as exc:
        assert str(exc) == "pipesize must be an integer"
    else:
        raise AssertionError("invalid pipesize accepted without a pipe")

    class IntSubclass(int):
        pass

    for redirect, exception in ((subprocess.STDOUT, ValueError), (int("-2"), ValueError),
                                (IntSubclass(-2), TypeError)):
        try:
            subprocess.Popen(["/bin/true"], stdout=redirect, pipesize="invalid")
        except (ValueError, TypeError) as exc:
            assert type(exc) is exception
        else:
            raise AssertionError("invalid stdout and pipesize accepted")

    # An empty read1 buffer performs just one raw read, without prefetching.
    with subprocess.Popen(["/bin/sh", "-c", "printf abcdefgh"], stdout=pipe, bufsize=4) as child:
        assert child.wait() == 0
        reader = child.stdout
        assert reader.read1(1) == b"a"
        assert os.read(reader.fileno(), 1) == b"b"
        assert reader.read(1) == b"c"
        assert reader.read1(64) == b"def"
        assert reader.read1(64) == b"gh"
        assert reader.read1() == b""

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always")
        with subprocess.Popen(["/bin/true"], bufsize=1, stdin=pipe, stdout=pipe) as child:
            child.wait()
        assert len(seen) == 2
        assert all(w.category is RuntimeWarning for w in seen)

    # Warning errors abort before exec. The child command must not create a file.
    with warnings.catch_warnings():
        warnings.simplefilter("error", RuntimeWarning)
        try:
            subprocess.Popen(["/bin/sh", "-c", "echo started > buffered_warning_child"], bufsize=1, stdout=pipe)
        except RuntimeWarning:
            pass
        else:
            raise AssertionError("binary line-buffering warning was ignored")
        assert not os.path.exists("buffered_warning_child")

    # Single-pipe communicate reads from the same buffer as the public stream.
    with subprocess.Popen(["/bin/printf", "ab\\ncd"], stdout=pipe, bufsize=4) as child:
        assert child.stdout.readline(2) == b"ab"
        assert child.communicate() == (b"\ncd", None)

    # Closing the writer sends its pending bytes and EOF to the child.
    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe, bufsize=8) as child:
        child.stdin.write(b"queued")
        child.stdin.close()
        assert child.stdout.read() == b"queued"
        assert child.wait() == 0

    # Closing the underlying FileIO discards pending high-level writes.
    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe, bufsize=8) as child:
        child.stdin.write(b"pending")
        child.stdin.raw.close()
        assert child.stdin.closed and child.stdout.read() == b""
        assert child.wait() == 0

    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe) as child:
        child.stdin.write(b"prefix")
        try:
            child.communicate("not bytes")
        except TypeError:
            pass
        else:
            raise AssertionError("binary stdin accepted text")
        os.write(child.stdin.fileno(), b"tail")
        child.stdin.close()
        assert child.stdout.read() == b"prefixtail"
        assert child.wait() == 0

    # A resumed exchange retains both the buffered prefix and the unsent input.
    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe, stderr=pipe) as child:
        child.stdin.write(b"prefix")
        try:
            child.communicate(b"tail", timeout=0)
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("zero timeout unexpectedly completed")
        assert child.communicate() == (b"prefixtail", b"")
        assert child.returncode == 0

    # After the child exits, communicate suppresses EPIPE from the buffered flush.
    with subprocess.Popen(["/bin/true"], stdin=pipe) as child:
        assert child.wait() == 0 and not child.stdin.closed
        assert child.stdin.write(b"x") == 1
        assert child.communicate() == (None, None)
        assert child.stdin.closed

    # Dropping the only public writer must let cat see EOF.
    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe) as child:
        child.stdin = None
        gc.collect()
        try:
            assert child.communicate(timeout=5) == (b"", None)
        except subprocess.TimeoutExpired:
            child.kill()
            raise
        assert child.returncode == 0

    # A timed-out exchange retains the caller's input until the retry writes it.
    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe) as child:
        data = bytearray(b"before")
        try:
            child.communicate(memoryview(data), timeout=0)
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("zero timeout completed the pipe write")
        data[:] = b"after!"
        assert child.communicate(timeout=5) == (b"after!", None)

    # communicate sends input to the replacement and leaves the original open.
    with subprocess.Popen(["/bin/true"], stdin=pipe, stdout=pipe) as child:
        original = child.stdin
        read_fd, write_fd = os.pipe()
        try:
            with os.fdopen(write_fd, "wb") as writer:
                child.stdin = writer
                data = bytearray(b"redirecteX")
                try:
                    child.communicate(data, timeout=0)
                except subprocess.TimeoutExpired:
                    pass
                else:
                    raise AssertionError("zero timeout completed the pipe write")
                data[-1] = ord("d")
                assert child.communicate(timeout=5) == (b"", None)
                assert writer.closed and not original.closed
                os.set_blocking(read_fd, False)
                assert os.read(read_fd, 64) == b"redirected"
                assert os.read(read_fd, 1) == b""
                assert child.returncode == 0
        finally:
            original.close()
            os.close(read_fd)

    with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe) as child:
        with os.fdopen(os.dup(child.stdin.fileno()), "wb") as writer:
            child.stdin.close()
            child.stdin = writer
            for data in (memoryview(b"abcdef")[::2], None):
                try:
                    child.communicate(data, timeout=5)
                except TypeError:
                    pass
                else:
                    raise AssertionError("replacement stdin accepted a strided view")
                assert not writer.closed
            writer.write(b"still open")
        assert child.stdout.read() == b"still open"
        assert child.wait() == 0

    # Exercise the run_python_until_end pattern, including cleanup after wait.
    with subprocess.Popen(["/bin/sh", "-c", "cat; printf err >&2; exit 7"],
                          stdin=pipe, stdout=pipe, stderr=pipe,
                          env=os.environ.copy(), cwd=None) as child:
        try:
            assert child.communicate(b"in") == (b"in", b"err")
        finally:
            child.kill()
            subprocess._cleanup()
        assert child.returncode == 7

    print("buffered Popen streams passed")
