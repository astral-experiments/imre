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
