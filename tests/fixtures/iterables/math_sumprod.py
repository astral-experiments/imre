from math import sumprod


def integers(p: list[int], q: list[int]) -> int:
    return sumprod(p, q)


def float_total(p: list[float], q: list[float]) -> object:
    return sumprod(p, q)


def left(values: list[int], events: list[str]) -> list[int]:
    events.append("left")
    return values


def right(values: list[int], events: list[str]) -> list[int]:
    events.append("right")
    values.append(3)
    return [7, 11]


huge = 1 << 180
print(integers([huge + 1, -huge], [huge - 1, huge]))
print(integers([], []))
same = [huge, -3]
print(integers(same, same) == huge * huge + 9)
wide = [1 << 61] * 64
print(integers(wide, wide) == 1 << 128)
print(integers(wide + [-(1 << 61)] * 64, wide + wide) == 0)

empty = float_total([], [])
zero = float_total([0.0], [1.0])
print(repr(empty))
print(repr(zero))
print(isinstance(empty, int))
print(isinstance(zero, float))
same_floats = [1.5, -2.0]
print(float_total(same_floats, same_floats))

# Both ordinary binary64 products round to 2**54 in magnitude.
x = 134217728.0
print(sumprod([x + 1.0, -x], (x - 1.0, x)))

events: list[str] = []
values = [2]
print(sumprod(left(values, events), right(values, events)))
print(events)
try:
    print(integers(values, [1]))
except ValueError as exc:
    print(str(exc))
