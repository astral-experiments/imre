"""Binary subclasses inherit their live sequence length and truth value."""

class Packet(bytes):
    pass


class Buffer(bytearray):
    pass


class Sized(Buffer):
    def __len__(self):
        return 0


class Truthful(Sized):
    def __bool__(self):
        return True


def current():
    for cls in (Packet, Buffer):
        assert len(cls()) == 0 and not cls()
        assert len(cls(b"abc")) == 3 and cls(b"abc")
    data = Buffer(b"a")
    data.clear()
    assert len(data) == 0 and not data
    assert len(Sized(b"a")) == 0 and not Sized(b"a")
    assert len(Truthful()) == 0 and Truthful()

    class Oversized(Buffer):
        def __len__(self): return -(1 << 100)

    for measure in (len, bool):
        try:
            measure(Oversized())
        except ValueError:
            pass
        else:
            raise AssertionError("negative length accepted")
