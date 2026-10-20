def forward(text: str) -> int:
    values = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(values):
        digit = values[cursor]
        cursor += 1
        if digit == 0:
            continue
        total += digit * 2
    return total


def reverse(text: str) -> int:
    values = list(map(int, text))
    cursor = len(values) + -1
    total = 0
    while cursor >= 0:
        total += values[cursor] * 2
        cursor += -2
    return total


def after_advance(text: str) -> int:
    values = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(values):
        digit = values[cursor]
        cursor += 1
        total += digit + values[cursor]
    return total


def other_source(text: str) -> int:
    values = list(map(int, text))
    shorter = list(map(int, "4"))
    cursor = 0
    total = 0
    while cursor < len(values):
        total += values[cursor] + shorter[cursor]
        cursor += 1
    return total


def other_index(text: str) -> int:
    values = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(values):
        total += values[cursor] + values[cursor + 1]
        cursor += 1
    return total


def after_loop(text: str) -> int:
    values = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(values):
        digit = values[cursor]
        total += digit * 2
        cursor += 1
        if digit == 9:
            break
    else:
        return total + values[cursor]
    return total


def arbitrary_index(text: str, index: int) -> int:
    values = list(map(int, text))
    cursor = 0
    total = 0
    while cursor < len(values):
        total += values[cursor] + values[index]
        cursor += 1
    return total


def shrink(values: list[int], cursor: int) -> int:
    values.pop()
    return cursor


def mutation(values: list[int]) -> int:
    cursor = 0
    total = 0
    while cursor < len(values):
        total += values[shrink(values, cursor)] * 2
        cursor += 1
    return total


for text in ["", "0", "9", "0123", "98765"]:
    print("forward", text, forward(text))
    print("reverse", text, reverse(text))
    try:
        print("after", text, after_advance(text))
    except IndexError:
        print("after", text, "IndexError")
    try:
        print("source", text, other_source(text))
    except IndexError:
        print("source", text, "IndexError")
    try:
        print("index", text, other_index(text))
    except IndexError:
        print("index", text, "IndexError")
    try:
        print("else", text, after_loop(text))
    except IndexError:
        print("else", text, "IndexError")

for index in [-1, -5, 1 << 100]:
    try:
        print("arbitrary", index, arbitrary_index("1234", index))
    except IndexError:
        print("arbitrary", index, "IndexError")

for values in [[1], [1, 2, 3], [1, 2, 3, 4]]:
    try:
        print("mutation", mutation(values))
    except IndexError:
        print("mutation", "IndexError")
    print(values)
