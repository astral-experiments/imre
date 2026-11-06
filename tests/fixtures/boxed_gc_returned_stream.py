import os
import weakref


def check_returned_stream():
    events = []
    references = []

    class Argument:
        def __del__(self):
            events.append("finalized")

    def return_stream(stream, argument):
        references.append(weakref.ref(argument, lambda ref: events.append("weak")))
        return stream

    stream = return_stream(open(os.devnull, encoding="utf-8"), Argument())
    assert events == ["finalized", "weak"]
    assert references[0]() is None
    raw = stream.buffer.raw
    assert stream.read() == ""
    assert not raw.closed
    del stream
    assert raw.closed

    stream = open(os.devnull, encoding="utf-8")
    buffer = stream.detach()
    assert return_stream(stream, Argument()) is stream
    assert events == ["finalized", "weak", "finalized", "weak"]
    assert references[1]() is None
    assert not buffer.closed
    buffer.close()

    binary = return_stream(open(os.devnull, "rb"), Argument())
    assert references[-1]() is None
    descriptor = binary.fileno()
    assert binary.read() == b""
    del binary
    try:
        os.fstat(descriptor)
    except OSError:
        pass
    else:
        raise AssertionError("discarded file descriptor is still open")

    directory = os.path.dirname(os.path.abspath(__file__))
    with return_stream(os.scandir(directory), Argument()) as entries:
        assert next(entries).name
        assert references[-1]() is None

    owner = return_stream(Argument(), Argument())
    assert references[-1]() is None
    assert events == ["finalized", "weak"] * 5
    del owner
    assert events[-1] == "finalized"


def check_replaced_dictionary_edges():
    finalized = []

    class Resource:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            finalized.append(self.label)

    class Holder:
        def method(self):
            pass

    holder = Holder()
    holder.resource = Resource("first")
    assert holder.__dict__["resource"] is holder.resource
    method = holder.method
    del holder
    method()
    method.__self__.resource = Resource("second")
    assert finalized == ["first"]
    method.__self__.__dict__ = {}
    assert finalized == ["first", "second"]



check_returned_stream()
check_replaced_dictionary_edges()
print("returned stream cleanup ok")
