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


def check_global_reduction():
    import sys
    import types

    class Token:
        __module__ = None

        def __reduce__(self):
            return "holder.token"

        def __eq__(self, other):
            return isinstance(other, Token)

    first = types.ModuleType("pickle_shadow")
    second = types.ModuleType("pickle_target")
    sys.modules[first.__name__] = first
    sys.modules[second.__name__] = second
    try:
        first.holder = types.SimpleNamespace(token=Token())
        second.holder = types.SimpleNamespace(token=Token())
        token = second.holder.token
        for protocol in range(6):
            assert pickle.loads(pickle.dumps(token, protocol)) is token
    finally:
        del sys.modules[first.__name__]
        del sys.modules[second.__name__]


def check_frame_reader():
    def frame(opcodes):
        return pickle.FRAME + len(opcodes).to_bytes(8, "little") + opcodes

    # Two independent SHORT_BINBYTES opcodes from a stock protocol-5 pickle.
    a, b = pickle.loads(b"\x80\x05" + frame(b"C\x01x\x94C\x01x\x94\x86\x94."))
    assert a == b == b"x" and a is not b

    # Build a list across two frames, then load a separate pickle from the same stream.
    first = b"\x80\x04" + frame(b"](") + frame(b"K\x01I2\ne.")
    second = b"I42\n."
    stream = io.BytesIO(first + second)
    reader = pickle.Unpickler(stream)
    assert reader.load() == [1, 2] and stream.tell() == len(first)
    assert reader.load() == 42 and stream.tell() == len(first) + len(second)

    try:
        pickle.loads(b"\x80\x04" + frame(b"I42\n.")[:-2])
    except pickle.UnpicklingError:
        pass
    else:
        raise AssertionError("accepted a truncated pickle frame")


def current():
    check_frame_reader()
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
    check_global_reduction()
    print("pickle writer")


if __name__ == "__main__":
    current()
