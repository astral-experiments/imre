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
    for protocol in range(6):
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
    print("pickle writer")
