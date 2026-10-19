# ruff: noqa: PLW0120
# Unpacking failures must skip the loop's exhaustion suite.

first = -1
second = -2
third = -3
try:
    first, second = [10]
except ValueError:
    print("short")
print(first)
print(second)
try:
    first, second = [10, 20, 30]
except ValueError:
    print("long")
print(first)
print(second)

try:
    first, (second, third) = (10, [20])
except ValueError:
    print("nested short")
print(first)
print(second)
print(third)

fourth = -4
try:
    (first, second), (third, fourth) = [[31, 32], [33]]
except ValueError:
    print("second nested short")
print(first)
print(second)
print(third)
print(fourth)

rest = [40]
try:
    first, *rest, second = [41]
except ValueError:
    print("starred short")
print(first)
print(rest[0])
print(second)

values = [50]
try:
    first, values[5] = [51, 52]
except IndexError:
    print("store failed")
print(first)
print(values[0])

try:
    for first, second in [[61, 62], [63]]:
        print(first + second)
    else:
        print("unreachable else")
except ValueError:
    print("loop short")
print(first)
print(second)

try:
    [] = [70]
except ValueError:
    print("empty target rejects a value")

try:
    ((first, second),) = [[81]]
except ValueError:
    print("nested list length is checked at runtime")
print(first)
print(second)
