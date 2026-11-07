events = []


def record(label, value):
    events.append(label)
    return value


def target(a, /, b=0, *, café=0):
    return a, b, café


def selected():
    return record("callable", target)


def pack(**kwargs):
    return tuple(kwargs.items())


print(selected()(record("a", 1), café=record("café", 2), b=record("b", 3)))
print(events)
print(pack(outer=pack(inner=1), café=target(2, b=3, café=4)))


class Receiver:
    def method(self, positional, *, label):
        return positional, label


print(Receiver().method(5, label=pack(a=6, b=7)))
try:
    target(1, unknown=record("unknown", 8), b=record("last", 9))
except TypeError as error:
    print(type(error).__name__, events[-2:])
