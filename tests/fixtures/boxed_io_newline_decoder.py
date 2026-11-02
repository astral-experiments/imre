import _io
import io
import sys
import weakref
from io import IncrementalNewlineDecoder


class Delegate:
    def decode(self, text, final):
        if not text:
            raise RuntimeError("decode")
        assert type(final) is bool
        return text

    def getstate(self):
        return self, -1

    def setstate(self, state):
        assert state == (self, (1 << 63) - 1)
        return self


class Derived(IncrementalNewlineDecoder):
    pass


class Text(str):
    pass


def current():
    decoder = io.IncrementalNewlineDecoder
    assert decoder is _io.IncrementalNewlineDecoder
    delegate = Delegate()
    wrapped = decoder(delegate, True)
    text = Text("x\ud800\n")
    assert wrapped.decode(text) is text
    assert wrapped.decode("\r") == ""
    try:
        wrapped.decode("")
    except RuntimeError as exc:
        assert str(exc) == "decode"
    else:
        raise AssertionError("expected delegate failure")
    assert wrapped.getstate() == (delegate, (1 << 64) - 1)
    assert wrapped.decode("\n\r\x00") == "\n\n\x00"
    assert wrapped.newlines == ("\r", "\n", "\r\n")
    assert type(wrapped.decode(Text("x"))) is str
    clean = Derived(None, True)
    try:
        decoder.reset(weakref.proxy(clean))
    except TypeError as exc:
        assert "weakref.ProxyType" in str(exc)
    else:
        raise AssertionError("unbound descriptor accepted a proxy")
    clean.tag = "subclass"
    assert clean.tag == "subclass" and type(clean.decode(Text("\u0d00"))) is str
    assert wrapped.setstate((delegate, -1)) is delegate
    assert wrapped.decode("x") == "\nx"
    uninitialized = decoder.__new__(decoder)
    try:
        uninitialized.reset()
    except ValueError as exc:
        assert str(exc) == "IncrementalNewlineDecoder.__init__() not called"
    else:
        raise AssertionError("expected uninitialized decoder")

    try:
        del sys.modules["io"]
        public = __import__("io")
        assert public.IncrementalNewlineDecoder is decoder
        del sys.modules["_io"]
        fresh = __import__("_io").IncrementalNewlineDecoder
        assert fresh is not decoder and public.IncrementalNewlineDecoder is decoder
        assert wrapped.decode("x") == "x" and fresh(None, False).decode("y") == "y"
        del sys.modules["io"]
        assert __import__("io").IncrementalNewlineDecoder is fresh
        native = sys.modules["_io"]
        del native.IncrementalNewlineDecoder
        del sys.modules["io"]
        try:
            __import__("io")
        except ImportError as exc:
            assert exc.name == "_io" and "io" not in sys.modules
        else:
            raise AssertionError("missing decoder import succeeded")
        native.IncrementalNewlineDecoder = fresh
        assert __import__("io").IncrementalNewlineDecoder is fresh
    finally:
        sys.modules["io"] = io
        sys.modules["_io"] = _io
