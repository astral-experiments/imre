import _io
import io
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
