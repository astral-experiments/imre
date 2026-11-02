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
with io.BytesIO(b"abcdef") as raw:
    f = io.BufferedRandom(raw, 3)
    assert f.read(1) == b"a"
    raw.seek(1)
    raw.write(b"X")
    raw.seek(3)
    assert f.peek() == b"Xcd"
    f.seek(0)
    assert f.write(b"ABC") == 3
    assert raw.getvalue() == b"ABCdef"
    f.close()


class Reenter(io.BytesIO):
    target = None

    @property
    def closed(self):
        if self.target is not None:
            target = self.target
            self.target = None
            try:
                target.write(b"X")
            except RuntimeError:
                self.rejected = True
            else:
                raise AssertionError("reentrant write accepted")
        return super().closed


with Reenter() as raw:
    f = io.BufferedRandom(raw, 4)
    raw.target = f
    assert f.write(b"A") == 1 and raw.rejected
    f.flush()
    assert raw.getvalue() == b"A"
    f.close()
for method in ("readinto", "readinto1"):
    with io.BytesIO() as raw:
        f = io.BufferedRandom(raw, 4)
        assert f.write(b"XY") == 2 and raw.getvalue() == b""
        assert getattr(f, method)(bytearray(0)) == 0
        assert raw.getvalue() == b"XY"
        f.close()


class BlockedRaw(io.BytesIO):
    blocked = False

    def write(self, data):
        if self.blocked:
            return None
        return super().write(data)


with BlockedRaw(b"abcdefgh") as raw:
    f = io.BufferedRandom(raw, 6)
    assert f.read(3) == b"abc" and f.write(b"X") == 1 and f.seek(1) == 1
    raw.blocked = True
    try:
        f.write(b"01234567")
    except BlockingIOError as error:
        assert error.characters_written == 5
    else:
        raise AssertionError("blocked preflush accepted too much")
    assert f.tell() == 6
    raw.blocked = False
    f.flush()
    assert f.tell() == raw.tell() == 9
    assert raw.getvalue() == b"abcX01234"
    assert f.write(b"Z") == 1
    f.flush()
    assert raw.getvalue() == b"abcX01234Z"
    assert f.seek(8) == 8 and f.read() == b"4Z"
    f.close()

# Older native TemporaryFile wrappers own their descriptor internally.
# A refused detach must leave the stream alive and closeable.
temporary = tempfile.TemporaryFile("w+b")
try:
    raw = temporary.detach()
except io.UnsupportedOperation:
    assert not temporary.closed
    temporary.close()
else:
    assert not raw.closed
    raw.close()
