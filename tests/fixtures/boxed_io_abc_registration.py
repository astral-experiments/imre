import _io
import abc
import io


def current():
    assert io.IOBase.__bases__ == (_io._IOBase,)
    for public, private in ((io.RawIOBase, _io._RawIOBase),
                            (io.BufferedIOBase, _io._BufferedIOBase),
                            (io.TextIOBase, _io._TextIOBase)):
        assert public.__bases__ == (private, io.IOBase)
        assert type(public) is abc.ABCMeta

    class Stream:
        pass

    class Child(Stream):
        pass

    assert not issubclass(Stream, io.IOBase)
    before = abc.get_cache_token()
    assert io.RawIOBase.register(Stream) is Stream
    registered = abc.get_cache_token()
    assert registered != before
    assert io.RawIOBase.register(Stream) is Stream
    assert abc.get_cache_token() == registered
    assert issubclass(Stream, io.RawIOBase) and isinstance(Child(), io.IOBase)
    assert io.IOBase not in Child.__mro__
    try:
        io.RawIOBase.register(io.IOBase)
    except RuntimeError:
        pass
    else:
        raise AssertionError("registration introduced an inheritance cycle")
    assert not issubclass(io.IOBase, io.RawIOBase)
    assert isinstance(io.BytesIO(b"data"), io.BufferedIOBase)
