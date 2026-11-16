import sys


events = list(sys.argv[1:])


def record(label, value):
    events.append(label)
    return value


def target(a, /, b=0, *, café=0):
    return a, b, café


def selected():
    return record("callable", target)


def pack(**kwargs):
    return tuple(kwargs.items())


class Receiver:
    def method(self, positional, *, label):
        return positional, label


class TemporarySpread:
    def __iter__(self):
        return record("iterate", iter(()))

    def __del__(self):
        events.append("release star")


class TemporaryMapping:
    def keys(self):
        return ()

    def __del__(self):
        events.append("release mapping")


def observe(*args, **kwargs):
    events.append("enter")


def current():
    events.clear()
    # The discarded list elements die before evaluation advances past the
    # callable or a written keyword. The selected value survives the lookup.
    ([TemporaryMapping(), observe][1])(
        first=[TemporaryMapping(), 3][1], later=record("later", 1)
    )
    assert events == ["release mapping", "release mapping", "later", "enter"]
    events.clear()
    receiver = Receiver()
    receiver.method([TemporaryMapping(), 3][1], label=record("method", 1))
    assert events == ["release mapping", "method"]
    events.clear()
    try:
        receiver.method([TemporaryMapping(), 3][1], label=1 // 0)
    except ZeroDivisionError:
        events.append("handler")
    assert events == ["release mapping", "handler"]
    events.clear()
    observe(*TemporarySpread(), **TemporaryMapping(), later=record("later", 1))
    assert events == ["release mapping", "later", "iterate", "release star", "enter"]


print(selected()(record("a", 1), café=record("café", 2), b=record("b", 3)))
print(events)
print(pack(outer=pack(inner=1), café=target(2, b=3, café=4)))

print(Receiver().method(5, label=pack(a=6, b=7)))
try:
    target(1, unknown=record("unknown", 8), b=record("last", 9))
except TypeError as error:
    print(type(error).__name__, events[-2:])

if __name__ == "__main__":
    current()
