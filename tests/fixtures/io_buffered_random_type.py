import io
import os
import sys
import tempfile

r = io.BufferedRandom
path="random-roundtrip"
class Derived(r):
    pass
class Size:
    def __index__(self):
        return 4
try:
    with io.FileIO(path, "w+") as raw, Derived(raw, Size()) as f:
        assert f.raw is raw and f.mode == "rb+" and f.name is raw.name
        assert f.write(b"abc") == 3 and raw.tell() == 0
        assert f.tell() == 3
        f.seek(0)
        assert f.read(1) == b"a" and f.tell() == 1
        assert f.write(b"Z") == 1
        f.seek(0)
        assert f.read() == b"aZc"
        assert f.truncate(2) == 2 and f.tell() == 3
        f.seek(0)
        assert f.read() == b"aZ"
    assert raw.closed and f.closed
    for context in (Derived(io.FileIO(path,"r+"), 4), tempfile.TemporaryFile("w+b")):
        marker = RuntimeError("context body")
        try:
            with context as stream:
                assert stream is context
                raise marker
        except RuntimeError as caught:
            assert caught is marker and context.closed
        else: raise AssertionError("exception suppressed")
        try: context.__enter__()
        except ValueError: pass
        else: raise AssertionError("closed stream entered")
    with Derived(io.FileIO(path,"r+"), 8) as guarded:
        guarded.write(b"aZc")
        class Position:
            def __index__(self):
                assert guarded.tell() == 3 and guarded.fileno() == guarded.raw.fileno()
                assert guarded.readable() and guarded.writable() and guarded.seekable()
                for attempt in (guarded.read1, guarded.flush, guarded.close):
                    try: attempt()
                    except RuntimeError as e: assert "reentrant" in str(e)
                    else: raise AssertionError("reentrant buffer access was accepted")
                return 2
        size = Position()
        assert guarded.truncate(size) is size and guarded.tell() == 3
        class BadPosition:
            def __index__(self):
                raise LookupError("size conversion")
        try: guarded.truncate(BadPosition())
        except LookupError as error: assert str(error) == "size conversion"
        else: raise AssertionError("size conversion exception lost")
        assert guarded.seek(0) == 0 and guarded.read() == b"aZ"
    with io.open(path, "rb+") as opened:
        assert type(opened) is r and opened.read() == b"aZ"
    with tempfile.TemporaryFile("w+b") as tf:
        assert type(tf) is r
        tf.writelines([b"a\n",b"c\n"])
        tf.seek(0)
        assert tf.peek(0) == b"a\nc\n" and tf.tell() == 0
        assert tf.read1(sys.maxsize) == b"a\nc\n"
        tf.seek(0)
        assert list(tf) == [b"a\n",b"c\n"]
        assert r.truncate(tf,2) == 2
    with io.FileIO(path, "r") as read:
        try:
            r(read)
        except io.UnsupportedOperation as e:
            assert str(e) == 'File or stream is not writable.'
        else: raise AssertionError("read-only raw accepted")
    with io.FileIO(path,"r+") as raw:
        try:r(raw,0)
        except ValueError as e: assert str(e) == "buffer size must be strictly positive"
        else: raise AssertionError("zero size accepted")
    try: r.read(object())
    except TypeError as e: assert "BufferedRandom" in str(e)
    else: raise AssertionError("wrong receiver accepted")
    with io.FileIO(path, "w+") as raw:
        raw.write(b"abcdef")
        raw.seek(0)
        with r(raw, 3) as stream:
            assert stream.read(1) == b"a" and raw.tell() == 3
            assert stream.peek(99) == b"bcd"
            assert stream.tell() == 1 and raw.tell() == 4
            assert stream.write(b"Z") == 1 and stream.peek(0) == b"cde"
            assert stream.tell() == 2 and raw.tell() == 5
            raw_truncate = raw.truncate
            calls = []
            def truncate(size):
                calls.append((size, raw.tell()))
                return raw_truncate(size)
            raw.truncate = truncate
            assert stream.truncate(2) == 2 and calls == [(2, 2)]
            assert stream.seek(0) == 0 and stream.peek() == b"aZ"
            assert stream.read1(sys.maxsize) == b"aZ" and stream.tell() == 2
        assert raw.closed
    oldio = sys.modules.pop("_io")
    try:
        freshio = __import__("_io")
        with freshio.open(path,"rb+") as f:
            assert type(f) is freshio.BufferedRandom and freshio.BufferedRandom is not r
            assert f.read() == b"aZ"
            try:r.read(f)
            except TypeError: pass
            else: raise AssertionError("old descriptor accepted fresh receiver")
    finally:
        sys.modules["_io"] = oldio
finally:
    os.unlink(path)
print("BufferedRandom checked")


class ShortRaw(io.BytesIO):
    def __init__(self, data):
        super().__init__(data)
        self.writes = []
        self.refuse = False

    def write(self, data):
        if self.refuse:
            self.refuse = False
            raise OSError("retry me")
        count = min(len(data), 2)
        self.writes.append((self.tell(), bytes(data[:count])))
        return super().write(data[:count])


with ShortRaw(b"abcdefgh") as raw:
    f = io.BufferedRandom(raw, 5)
    assert f.raw is raw and f.read(1) == b"a" and raw.tell() == 5
    assert f.write(b"123") == 3 and raw.writes == []
    assert f.seek(-2, 1) == 2 and f.read1(2) == b"23"
    raw.refuse = True
    try:
        f.flush()
    except OSError as error:
        assert error.args == ("retry me",)
    else:
        raise AssertionError("raw write failure lost")
    f.flush()
    assert raw.writes == [(1, b"12"), (3, b"3")]
    assert raw.tell() == 4 and raw.getvalue() == b"a123efgh"
    assert f.detach() is raw and not raw.closed
print("BufferedRandom raw callbacks checked")
