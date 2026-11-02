"""Strip byte values at each end of an immutable or mutable binary sequence."""

class Packet(bytes):
    pass


class Buffer(bytearray):
    pass


def current():
    for base, subtype in ((bytes, Packet), (bytearray, Buffer)):
        value = base(b"\v\x85x\xff\f")
        assert value.strip() == b"\x85x\xff"
        assert value.lstrip(None) == b"\x85x\xff\f"
        assert value.rstrip() == b"\v\x85x\xff"
        assert value.strip(b"") == value
        assert (value.strip(b"") is value) == (base is bytes)
        for chars in (b"ab", bytearray(b"ab"), memoryview(b"ab")):
            for name, expected in (("strip", b"x"), ("lstrip", b"xabab"), ("rstrip", b"babax")):
                result = getattr(base, name)(subtype(b"babaxabab"), chars)
                assert type(result) is base and result == expected
        derived = subtype(b"unchanged")
        first, second = derived.strip(), derived.strip()
        assert type(first) is base and first is not second and first == derived
        for name in ("strip", "lstrip", "rstrip"):
            method = getattr(derived, name)
            for args, kwargs, expected in (
                ((1,), {}, "a bytes-like object is required, not 'int'"),
                ((b"a", b"b"), {}, name + " expected at most 1 argument, got 2"),
                ((), {"chars": b"a"}, subtype.__name__ + "." + name + "() takes no keyword arguments"),
            ):
                try:
                    method(*args, **kwargs)
                except TypeError as ex:
                    assert str(ex) == expected
                else:
                    raise AssertionError((name, args, kwargs))
    print("binary strip matched")
