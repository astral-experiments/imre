import io


class Stream:
    pass


assert not issubclass(Stream, io.IOBase)
assert io.RawIOBase.register(Stream) is Stream
assert issubclass(Stream, io.RawIOBase)
assert isinstance(Stream(), io.IOBase)
