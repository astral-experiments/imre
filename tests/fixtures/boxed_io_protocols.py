import abc
import io
from io import Reader, Writer


def current():
    class Stream:
        def read(self, size=-1):
            return b"data"

        def write(self, data):
            return len(data)

    class Disabled(Stream):
        read = None

    class Concrete(Reader):
        def read(self, size=-1):
            return b"data"

    class Derived(Reader):
        pass

    for protocol, method in ((Reader, "read"), (Writer, "write")):
        assert type(protocol) is abc.ABCMeta
        assert protocol.__module__ == "io"
        assert protocol.__abstractmethods__ == frozenset({method})
        assert issubclass(Stream, protocol) and isinstance(Stream(), protocol)
        assert not issubclass(str, protocol)
        alias = protocol[bytes]
        assert type(alias) is type(list[bytes])
        assert alias.__origin__ is protocol and alias.__args__ == (bytes,)
        try:
            protocol()
        except TypeError as error:
            assert method in str(error)
        else:
            raise AssertionError("constructed an abstract protocol")

    assert not issubclass(Disabled, Reader)
    assert not issubclass(Stream, Derived)
    assert Concrete().read() == b"data"
    assert io.Writer is Writer
    io.Reader = Writer
    try:
        from io import Reader as rebound
        assert rebound is Writer and Reader is not Writer
    finally:
        io.Reader = Reader
    return "ok"
