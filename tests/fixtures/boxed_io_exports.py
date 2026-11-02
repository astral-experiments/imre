import _io
import io
import sys
from io import __all__ as exports


def current():
    assert type(exports) is list and exports is io.__all__
    assert exports == (
        "BlockingIOError open open_code IOBase RawIOBase FileIO BytesIO StringIO "
        "BufferedIOBase BufferedReader BufferedWriter BufferedRWPair BufferedRandom "
        "TextIOBase TextIOWrapper UnsupportedOperation SEEK_SET SEEK_CUR SEEK_END "
        "DEFAULT_BUFFER_SIZE text_encoding IncrementalNewlineDecoder Reader Writer"
    ).split()
    assert all(sys._is_interned(name) for name in exports)
    assert io.BlockingIOError is _io.BlockingIOError is BlockingIOError
    assert not hasattr(_io, "__all__")
    assert dir(io) == sorted(vars(io))
    open_code = io.open_code
    assert open_code is _io.open_code and "open_code" in dir(io)
    try:
        del io.open_code
        assert not hasattr(io, "open_code") and "open_code" not in dir(io)
        assert _io.open_code is open_code
    finally:
        io.open_code = open_code
    try:
        io.__all__ = ["SEEK_END"]
        namespace = {}
        exec("from io import *", namespace)
        assert namespace["SEEK_END"] == 2 and "open" not in namespace
        assert exports is not io.__all__
    finally:
        io.__all__ = exports
    return "ok"
