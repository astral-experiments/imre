import _io
import hashlib
import io
import sys


def current():
    class Stream(io.BytesIO):
        pass

    data = io.BytesIO(b"hash me")
    data.seek(3)
    child = Stream(b"child")
    text = io.StringIO("old")
    wrapper = io.TextIOWrapper(io.BytesIO(b"text"), encoding="utf-8")
    raw = io.FileIO(__file__, "r")
    del sys.modules["io"], sys.modules["_io"]
    try:
        import _io as replacement
        import io as fresh

        assert fresh.BytesIO is replacement.BytesIO and replacement.BytesIO is not io.BytesIO
        assert hashlib.file_digest(data, "sha256").digest() == hashlib.sha256(b"hash me").digest()
        with data:
            assert data.read() == b"h me"
        with child:
            assert io.BytesIO.read(child) == b"child"
        with text:
            assert text.read() == "old"
        with wrapper:
            assert wrapper.read() == "text"
        with raw:
            assert raw.read(0) == b""
        assert data.closed and child.closed and text.closed and wrapper.closed and raw.closed
        assert repr(raw) == "<_io.FileIO [closed]>"
        assert type(sys.stderr) is io.TextIOWrapper
        assert io.TextIOWrapper.write(sys.stderr, "") == 0
        for read, stream in ((fresh.BytesIO.read, child), (io.BytesIO.read, fresh.BytesIO()),
                             (fresh.TextIOWrapper.flush, sys.stderr)):
            try:
                read(stream)
            except TypeError:
                pass
            else:
                raise AssertionError("descriptor accepted a foreign receiver")
    finally:
        sys.modules["io"] = io
        sys.modules["_io"] = _io
