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
    check_renderer_release()
    check_standard_flush()


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


def check_renderer_release():
    data = bytearray()

    class Renderer:
        def __init__(self):
            self.view = memoryview(data)

        def __call__(self):
            return "label"

        def __del__(self):
            self.view.release()

    class Slot:
        def __get__(self, instance, owner):
            return Renderer()

    class Prompt:
        __str__ = Slot()

    class Stream:
        def write(self, text):
            if text == "label":
                data.append(65)

        def flush(self):
            pass

        def readline(self):
            return "ok\n"

    stdin, stdout = sys.stdin, sys.stdout
    try:
        sys.stdin = sys.stdout = Stream()
        assert input(Prompt()) == "ok"
        print(Prompt(), end="")
    finally:
        sys.stdin, sys.stdout = stdin, stdout
    assert data == b"AA"


def check_standard_flush():
    calls = []

    class FlushKey:
        def __hash__(self):
            return hash("flush")

        def __eq__(self, other):
            return other == "flush"

    def flush():
        calls.append("flush")

    for key in ("flush", FlushKey()):
        sys.stdout.__dict__[key] = flush
        try:
            print("", end="", flush=True)
        finally:
            del sys.stdout.__dict__[key]
    assert calls == ["flush", "flush"]

    def fileno():
        calls.append("fileno")
        return -1

    def readline():
        calls.append("readline")
        return "rebound\n"

    sys.stdin.fileno = fileno
    sys.stdin.readline = readline
    try:
        assert input() == "rebound"
    finally:
        del sys.stdin.readline
        del sys.stdin.fileno
    assert calls[-2:] == ["fileno", "readline"]


if __name__ == "__main__":
    current()
