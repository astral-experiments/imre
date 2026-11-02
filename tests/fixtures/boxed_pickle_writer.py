import io
import pickle


class Item:
    pass


def reduce_item(item):
    return list, ((4, 2),)


class Writer(pickle.Pickler):
    dispatch_table = {Item: reduce_item}


class Sink:
    def __init__(self):
        self.chunks = []

    def write(self, chunk):
        self.chunks.append(chunk)


def current():
    assert pickle.bytes_types == (bytes, bytearray)
    for name in ("Pickler", "Unpickler", "dump", "load", "dumps", "loads", "HIGHEST_PROTOCOL", "STOP"):
        assert name in pickle.__all__ and hasattr(pickle, name)
    assert "bytes_types" not in pickle.__all__
    for protocol in range(6):
        stream = io.BytesIO()
        assert pickle.dump((b"head", 3), stream, protocol) is None
        stream.write(pickle._dumps(b"tail", protocol))
        stream.seek(0)
        assert pickle.load(stream) == (b"head", 3)
        assert pickle.Unpickler(stream).load() == b"tail"
        try:
            pickle.load(stream)
        except EOFError:
            pass
        else:
            raise AssertionError("accepted an exhausted pickle stream")

        sink = Sink()
        writer = Writer(sink, protocol=protocol)
        obj = [Item()]
        assert writer.dump(obj) is None
        first = b''.join(sink.chunks)
        writer.dump(obj)
        reader = pickle._Unpickler(io.BytesIO(b''.join(sink.chunks)))
        value = reader.load()
        assert value == [[4, 2]] and reader.load() is value
        writer.clear_memo()
        sink.chunks.clear()
        writer.dump(obj)
        assert b''.join(sink.chunks) == first

    try:
        pickle.Pickler(io.BytesIO(), 4, buffer_callback=lambda value: None)
    except ValueError:
        pass
    else:
        raise AssertionError("accepted a buffer callback with protocol 4")
    class ReentrantSink:
        def __init__(self):
            self.nested = False

        def write(self, data):
            if isinstance(data, memoryview) and not self.nested:
                self.nested = True
                data.release()
                self.writer.dump(None)

    out = ReentrantSink()
    out.writer = pickle._Pickler(out, 5)
    out.writer.dump([1, 2])
    assert out.nested
    print("pickle writer")


if __name__ == "__main__":
    current()
