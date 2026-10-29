"""Inherited bytes and bytearray iterators read the receiver's native storage."""

import weakref


class Packet(bytes):
    def __getitem__(self, index):
        raise AssertionError("the inherited iterator called __getitem__")


class Buffer(bytearray):
    pass


class Custom(Buffer):
    def __iter__(self):
        return iter([42])


def current():
    assert list(Packet(b"ab")) == [97, 98]
    assert tuple(Custom(b"ab")) == (42,)
    data = Buffer(b"ab")
    reference = weakref.ref(data)
    data[0] = 65
    del data[-1]
    data[1:] = b"b"
    cursor = iter(data)
    assert next(cursor) == 65
    data[1:] = b"cd"
    del data
    assert reference() is not None
    assert list(cursor) == [99, 100]
