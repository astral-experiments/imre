"""A callback result can release a buffer before input invokes the next callback."""

import sys


def current():
    data = bytearray()

    class ErrorStream:
        def flush(self):
            return memoryview(data)

    class Descriptor:
        def __init__(self):
            self.view = memoryview(data)

        def __index__(self):
            return -1

    class Reader:
        def fileno(self):
            data.append(1)
            return Descriptor()

        def readline(self):
            data.append(2)
            return "ok\n"

    stdin, stderr = sys.stdin, sys.stderr
    try:
        sys.stdin, sys.stderr = Reader(), ErrorStream()
        line = input()
    finally:
        sys.stdin, sys.stderr = stdin, stderr
    print(line, list(data))
    check_stream_release()


def check_stream_release():
    stdin, stdout, stderr = sys.stdin, sys.stdout, sys.stderr
    data = bytearray()

    class Writer:
        def __init__(self):
            self.view = memoryview(data)

        def __del__(self):
            self.view.release()

    class ErrorSink:
        def __del__(self):
            try:
                data.append(65)
            except BufferError:
                pass

    class Reader:
        def readline(self):
            sys.stdin = stdin
            sys.stdout = stdout
            sys.stderr = stderr
            return "ok\n"

    for reverse in (False, True):
        try:
            if reverse:
                sys.stderr = ErrorSink()
                sys.stdout = Writer()
                sys.stdin = Reader()
            else:
                sys.stdin = Reader()
                sys.stdout = Writer()
                sys.stderr = ErrorSink()
            assert input() == "ok"
        finally:
            sys.stdin, sys.stdout, sys.stderr = stdin, stdout, stderr
    assert data == b"AA", data


if __name__ == "__main__":
    current()
