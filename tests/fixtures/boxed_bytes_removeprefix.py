from array import array


class Packet(bytes):
    pass


def current():
    data = b"\xef\xbb\xbfline\n"
    bom = data[:3]
    for prefix in (bom, bytearray(bom), array("B", bom), memoryview(data)[:3]):
        assert data.removeprefix(prefix) == b"line\n"
    for prefix in (b"", b"line", data + b"x"):
        assert data.removeprefix(prefix) is data
    assert data.removeprefix(data) == b""
    assert b"ababx".removeprefix(b"ab") == b"abx"
    original = Packet(data)
    unchanged = bytes.removeprefix(original, b"")
    assert unchanged == original and type(unchanged) is bytes
    method = bytes.removeprefix
    assert method.__objclass__ is bytes and method.__text_signature__ == "($self, prefix, /)"
    for args, kwargs, error in (
        ((), {}, TypeError),
        ((1,), {}, TypeError),
        ((bom, bom), {}, TypeError),
        ((), {"prefix": bom}, TypeError),
        ((memoryview(data)[::2],), {}, BufferError),
    ):
        try:
            data.removeprefix(*args, **kwargs)
        except error:
            pass
        else:
            raise AssertionError("invalid prefix or arguments")
