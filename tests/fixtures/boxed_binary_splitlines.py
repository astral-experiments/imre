"""Exercise binary record boundaries and keepends truth conversion."""


class Data(bytes):
    pass


class Buffer(bytearray):
    pass


class IndexOnly:
    def __index__(self):
        raise AssertionError("splitlines called __index__")


class BadTruth:
    def __bool__(self):
        raise LookupError("keepends")


def current():
    for ctor, base in ((bytes, bytes), (bytearray, bytearray), (Data, bytes), (Buffer, bytearray)):
        for raw, bare, kept in (
            (b"", [], []),
            (b"\r\n\n\r", [b"", b"", b""], [b"\r\n", b"\n", b"\r"]),
            (b"a\rb\nc\r\nd", [b"a", b"b", b"c", b"d"], [b"a\r", b"b\n", b"c\r\n", b"d"]),
            (b"\x00\x0b\x0c\x1c\x1d\x1e\x85\xff", [b"\x00\x0b\x0c\x1c\x1d\x1e\x85\xff"], [b"\x00\x0b\x0c\x1c\x1d\x1e\x85\xff"]),
        ):
            value = ctor(raw)
            assert value.splitlines() == bare
            for keep, expected in ((None, bare), ([], bare), ("yes", kept), (IndexOnly(), kept)):
                result = base.splitlines(value, keepends=keep)
                assert result == expected and all(type(line) is base for line in result)
        value = ctor(b"one\r\n")
        assert (value.splitlines(True)[0] is value) == (ctor is bytes)
        try:
            value.splitlines(BadTruth())
        except LookupError as exc:
            assert str(exc) == "keepends"
        else:
            raise AssertionError("ignored truth failure")
        for args, kwargs in (((), {"unknown": 1}), ((1, 2), {}), ((1,), {"keepends": 2})):
            try:
                value.splitlines(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))
            else:
                raise AssertionError("invalid argument accepted")

    buf = bytearray(b"before\r")

    class Mutate:
        def __bool__(self):
            buf.extend(b"\nafter")
            return True

    lines = buf.splitlines(Mutate())
    assert lines == [b"before\r\n", b"after"]
    lines[0].clear()
    assert buf == b"before\r\nafter"
    print("binary splitlines matched")
