def observe(log: list[int], value: int) -> int:
    log.append(value)
    if value == 9:
        raise ValueError("unexpected lazy element")
    return value


log: list[int] = []
print(any(observe(log, value) for value in [0, 2, 9]))
for value in log:
    print(value)
log = []
print(all(observe(log, value) for value in [2, 0, 9]))
for value in log:
    print(value)
print(any(map(int, "1x")))
print(all(map(int, "0x")))
print(any(value for value in map(int, "1x")))
print(all(value for value in map(int, "0x")))

# Strict zip only checks lengths when another item is requested.
print(any(zip([1], [2, 3], strict=True)))
print(any(left == right for left, right in zip([1], [1, 2], strict=True)))
print(all(left == right for left, right in zip([1], [2, 3], strict=True)))
try:
    print(all(zip([1], [2, 3], strict=True)))
except ValueError as error:
    print(error)
try:
    print(any(left == right for left, right in zip([1], [2, 3], strict=True)))
except ValueError as error:
    print(error)

# Unselected inner sources and filters stay lazy, including invalid range steps.
print(any(outer for outer in [1, 2] if outer))
print(any(inner for outer in [1, 0] for inner in range(1, 3, outer)))
print(all(inner for outer in [1, 0] for inner in range(0, 3, outer)))

# The later zip source is constructed even when the first source is empty.
try:
    print(any(zip([], range(0, 3, 0))))
except ValueError as error:
    print(error)
# But an element computation in that source is not pulled by non-strict zip.
print(any(zip([], (1 // value for value in [0]))))
print(all(zip([], (1 // value for value in [0]))))

log = []
for left, right in zip((observe(log, value) for value in [2, 9]), [3, 4], strict=True):
    print(left + right)
    break
for value in log:
    print(value)
