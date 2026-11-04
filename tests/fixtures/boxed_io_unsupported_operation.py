import _io
import io
import os
import sys


def current():
    initial = io.UnsupportedOperation
    assert initial is _io.UnsupportedOperation
    assert initial.__bases__ == (OSError, ValueError)
    assert repr(initial) == "<class 'io.UnsupportedOperation'>"
    exc = initial(11, "operation")
    assert type(exc) is initial and exc.errno == 11
    assert isinstance(exc, OSError) and isinstance(exc, ValueError)

    class Derived(initial):
        pass

    derived = Derived(11, "operation")
    assert type(derived) is Derived and isinstance(derived, initial)
    try:
        initial.__module__ = "custom_io"
        assert _io.UnsupportedOperation.__dict__["__module__"] == "custom_io"
    finally:
        initial.__module__ = "io"

    del sys.modules["_io"]
    try:
        fresh = __import__("_io")
        current_error = fresh.UnsupportedOperation
        assert current_error is not initial and current_error.__module__ == "io"
        fresh.UnsupportedOperation = LookupError
        for method, expected in (
            (io.BufferedIOBase().detach, initial),
            (fresh._BufferedIOBase().detach, current_error),
            (io.BytesIO().fileno, initial),
            (fresh.BytesIO().fileno, current_error),
        ):
            try:
                method()
            except expected as raised:
                assert type(raised) is expected
            else:
                raise AssertionError("missing native stream error")
        with fresh.open(os.devnull, "wb") as writer:
            assert type(writer) is fresh.BufferedWriter
            try:
                writer.read(1)
            except current_error as raised:
                assert type(raised) is current_error
            else:
                raise AssertionError("fresh writer accepted a read")
        random_type = fresh.BufferedRandom
        fresh.BufferedRandom = LookupError
        with fresh.open(os.devnull, "rb+") as stream:
            assert type(stream) is random_type and stream.readable() and stream.writable()
    finally:
        sys.modules["_io"] = _io
    return "ok"
