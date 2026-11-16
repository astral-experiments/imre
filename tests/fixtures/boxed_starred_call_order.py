import struct


events = []


class Source:
    def __iter__(self):
        events.append("iterate")
        yield 1


class Broken:
    def __iter__(self):
        events.append("bad-iter")
        raise ValueError("bad source")


def source():
    events.append("source")
    return Source()


def keyword():
    events.append("keyword")
    return 7


def capture(*args, **kwargs):
    events.append("call")
    return args, kwargs


def choose():
    events.append("callable")
    return capture


assert choose()(*source(), k=keyword()) == ((1,), {"k": 7})
assert events == ["callable", "source", "keyword", "iterate", "call"]

events.clear()
assert choose()(0, *source(), k=keyword()) == ((0, 1), {"k": 7})
assert events == ["callable", "source", "iterate", "keyword", "call"]

items = [1, 2, 3]
assert capture(*items, k=items.pop()) == ((1, 2), {"k": 3})

events.clear()
try:
    capture(*Broken(), k=1, **{"k": 2})
except TypeError:
    assert events == []
else:
    raise AssertionError("accepted a duplicate keyword")

events.clear()
try:
    capture(*Broken(), k=keyword())
except ValueError:
    assert events == ["keyword", "bad-iter"]
else:
    raise AssertionError("did not consume the starred iterable")

assert struct.Struct(">2B").pack(*(1, 2)) == bytes((1, 2))


def class_with_keywords(settings):
    class Packed(**settings):
        pass
    yield Packed.__name__


assert next(class_with_keywords({"metaclass": type})) == "Packed"
