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
    with io.open(path, "rb+") as opened:
        assert type(opened) is r and opened.read() == b"aZ"
    with tempfile.TemporaryFile("w+b") as tf:
        assert type(tf) is r
        tf.writelines([b"a\n",b"c\n"])
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
