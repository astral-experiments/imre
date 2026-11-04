def observe(value: int, visits: list[int]) -> int:
    visits.append(value)
    return value


def replace_item(values: list[int]) -> int:
    values[0] = 0
    return 1


large = 1000000000000000000000000000000
visits: list[int] = []
print(min(observe(n, visits) for n in [large, -large, -large]))
print(len(visits))
print(visits[0])
print(max(observe(n, visits) for n in [-large, 0, large] if n))
print(len(visits))
print(visits[-1])
print(max(i + j for i in [1, 2] for j in [3]))
try:
    min(n for n in [1] if n < 0)
except ValueError as error:
    print(str(error))
try:
    max(n for n in range(0))
except ValueError as error:
    print(str(error))
try:
    min(10 // observe(n, visits) for n in [2, 0, 7])
except ZeroDivisionError:
    print(visits[-1])

visits = []
print(max(observe(1, visits), observe(large, visits), observe(-large, visits)))
print(min(observe(-large, visits), observe(-large, visits)))
print(repr(visits))


# The first argument must survive a mutation performed by the next argument.
values = [-large - 1]
print(min(values[0], replace_item(values)))
visits = []
try:
    max(observe(1, visits), 1 // observe(0, visits), observe(7, visits))
except ZeroDivisionError:
    print(repr(visits))
