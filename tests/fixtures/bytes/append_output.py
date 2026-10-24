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


for value in [-1, 0, 255, 256, 1 << 100]:
    for late in [False, True]:
        try:
            print(encode(value, late))
        except (KeyError, ValueError) as error:
            print(error)
    print(repair(value))
