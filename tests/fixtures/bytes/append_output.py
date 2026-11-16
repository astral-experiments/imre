def encode(value: int, fail_late: bool) -> bytes:
    output: list[int] = []
    output.append(0)
    output.append(value)
    if fail_late:
        raise KeyError("later")
    return bytes(output)


def repair(value: int) -> bytes:
    output: list[int] = []
    output.append(value)
    alias = output
    alias[0] = 255
    return bytes(output)


def adjacent(data: list[int], late: bool) -> bytes:
    output: list[int] = []
    for i in range(1, len(data)):
        previous = data[i - 1]
        current = data[i]
        if current == 999999:
            raise KeyError("later input")
        output.extend(divmod(previous + current, 256))
    if late:
        raise KeyError("late")
    return bytes(output)


for value in [-1, 0, 255, 256, 1 << 100]:
    for late in [False, True]:
        try:
            print(encode(value, late))
        except (KeyError, ValueError) as error:
            print(error)
    print(repair(value))


for count in (0, 1, 257, 513):
    print(adjacent([i & 255 for i in range(count)], False))
for values in ([-1, 0], [0, 1 << 95], [0, -1, 999999]):
    for late in (False, True):
        try:
            print(adjacent(values, late))
        except (KeyError, ValueError) as error:
            print(error)
