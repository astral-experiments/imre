"""Range facts must describe current values without changing Python effects."""

# Boolean and chained guards intentionally exercise different IR forms.
# ruff: noqa: PLR1716


class Box:
    index: int
    values: list[int]

    def __init__(self, index: int, values: list[int]) -> None:
        self.index = index
        self.values = values


def branches(flag: bool) -> None:
    values = [10, 20, 30]
    index = 0
    if flag:
        index = -1
    print(values[index])
    if index < 0:
        index = 1
    else:
        index = -1
    print(values[index])
    if index >= 0:
        index -= 2
        print(values[index])


def loops() -> None:
    values = [10, 20, 30]
    index = 0
    turn = 0
    while turn < 2:
        print(values[index])
        index = -1
        turn += 1

    index = -2
    for index in range(0):
        pass
    print(values[index])
    for index in range(3):
        if index == 1:
            index = -1
            print(values[index])
            continue
        if index == 2:
            index = -2
            break
        print(values[index])
    print(values[index])
    for index in range(2, -1, -1):
        print(values[index])
    for index in range(-1, -4, -1):
        print(values[index])
    for index in range(2):
        for index in range(-1, 0):
            pass
        print(values[index])
    stop = 2
    for index in range(stop):
        stop = -1
        print(values[index])
    for index in range(4611686018427387903, 4611686018427387906):
        print(index)
    for index in range(-4611686018427387903, -4611686018427387906, -1):
        print(index)
    # All arguments fit the small representation; the final cursor step does not.
    for index in range(4611686018427387902, 4611686018427387903, 2):
        print(index)
    print(index)
    for index in range(-4611686018427387903, -4611686018427387904, -2):
        print(index)
    print(index)
    index = 18446744073709551616
    for index in range(5, 5):
        pass
    print(index)
    step = 1
    for index in range(0, 3, step):
        step = -2
        print(index)

    # The range keeps its original bounds; rebinding changes the indexed object.
    values = [1, 2]
    for index in range(len(values)):
        print(values[index])
        values = [3, 4]
    print(values[index])


def change(box: Box, indices: list[int]) -> bool:
    print("change")
    box.index = -1
    box.values = [40, 50]
    indices[0] = -1
    return True


def aliases() -> None:
    values = [10, 20]
    alias = values
    index = 0
    if 0 <= index and index < len(values):
        for extra in range(20):
            alias.append(extra)
        print(values[index])
        alias[index] = 99
        print(values[index])
    index = -1
    print(values[index])
    alias.pop()
    print(values[index])

    box = Box(0, values)
    indices = [0]
    saved_index = indices[0]
    saved_values = box.values
    if box.index >= 0 and change(box, indices):
        print(box.values[box.index])
        print(values[indices[0]])
        print(saved_values[saved_index])
    # Both operands of a short-circuit guard must retain their effects.
    if saved_index < 0 and change(box, indices):
        print(999)
    print(box.index)


def target(values: list[int], log: list[str]) -> list[int]:
    log.append("target")
    return values


def right(values: list[int], log: list[str]) -> int:
    log.append("right")
    values[0] = 0
    values.append(8)
    return 7


def assignments() -> None:
    values = [1000000000000000000000000000000]
    log: list[str] = []
    index = 0
    if 0 <= index and index < len(values):
        target(values, log)[index] = right(values, log)
    print(values[0])
    values = [1000000000000000000000000000000]
    if 0 <= index and index < len(values):
        # The owned old element survives the RHS replacing its last list slot.
        target(values, log)[index] += right(values, log)
    print(values[0])
    for position in range(len(log)):
        print(log[position])


def replace_first(values: list[int]) -> int:
    print("replace")
    values[0] = 0
    return 0


def replace_target(values: list[int]) -> list[int]:
    replace_first(values)
    return values


def comparisons() -> None:
    values = [1000000000000000000000000000000, 2000000000000000000000000000000]
    alias = values
    print(values[0] < alias[1])
    print(values[1] > alias[0])
    print(values[0] == alias[0])
    print(values[0] != alias[1])
    print(values[0] <= alias[0])
    print(values[1] >= alias[1])
    print(values[0] + alias[0] == values[1])
    print(values[-1] // 2 == alias[0])

    # A later operation may remove the only storage owner of the first operand.
    values = [1000000000000000000000000000000]
    alias = values
    print(values[0] > replace_first(alias))
    values = [1000000000000000000000000000000]
    print(values[0] > values.pop() - 1000000000000000000000000000000)
    # The middle operand must remain owned until the chain's final comparison.
    values = [1000000000000000000000000000000]
    print(-1 < values[0] > replace_first(values))
    values = [1000000000000000000000000000000]
    print(0 > values[0] > replace_first(values))
    print(values[0])


def copy_at(values: list[int], destination: int, source: int) -> None:
    values[destination] = values[source]


def copies() -> None:
    values = [0, 1000000000000000000000000000000]
    alias = values
    destination = 0
    source = 1
    values[destination] = values[source]
    alias.pop()
    print(values[0])
    # The destination is now the sole owner; acquire before replacing itself.
    values[destination] = values[destination]
    print(values.pop())

    values = [0, 1000000000000000000000000000001]
    alias = values
    alias[0] = values[1]
    values.pop()
    print(alias[0])
    values = [0, 1000000000000000000000000000002]
    copy_at(values, -2, -1)
    values.pop()
    print(values[0])

    # RHS ownership precedes either target call, which removes its old owner.
    values = [1000000000000000000000000000003]
    replace_target(values)[0] = values[0]
    print(values[0])
    values[replace_first(values)] = values[0]
    print(values[0])
    values[0] = values[replace_first(values)]
    print(values[0])

    # Local index proofs do not freeze the list handle or its stored values.
    values = [0, 1]
    alias = values
    if 0 <= destination < len(values) and replace_first(values) == 0:
        values = [1000000000000000000000000000004, 2]
        values[source] = values[destination]
        print(values[source])
        print(alias[source])


def ownership() -> None:
    index = 0
    texts = ["retained text with a separate character buffer"]
    if 0 <= index and index < len(texts):
        saved_text = texts[index]
        texts[index] = "replacement"
        texts.pop()
        print(saved_text)
    boxes = [Box(17, [19])]
    if 0 <= index < len(boxes):
        saved_box = boxes[index]
        boxes[index] = Box(23, [])
        boxes.pop()
        print(saved_box.index)
        print(saved_box.values[0])
    rows = [[29]]
    if 0 <= index < len(rows):
        saved_row = rows[index]
        rows[index] = []
        rows.pop()
        print(saved_row[0])
    flags = [True, False]
    for index in range(len(flags)):
        flags[index] = not flags[index]
        print(flags[index])
    index = 0
    pairs: list[tuple[str, int]] = [("retained tuple", 1000000000000000000000000000000)]
    if 0 <= index < len(pairs):
        saved_pair = pairs[index]
        pairs[index] = ("replacement", 0)
        pairs.pop()
        print(saved_pair[0])
        print(saved_pair[1])
    optional_texts: list[str | None] = ["retained optional text", None]
    if 0 <= index < len(optional_texts):
        saved_optional = optional_texts[index]
        optional_texts[index] = None
        optional_texts.pop()
        if saved_optional is not None:
            print(saved_optional)
        empty = optional_texts[index]
        print(empty is None)


def arithmetic(value: int) -> None:
    if -4611686018427387904 <= value and value <= 4611686018427387903:
        print(value + 1)
        print(value - 1)
        print(-value)
        print(value * 2)
        print(value * value)
        print(value // -1)
        print(value % -1)
    if value >= 0:
        print(value < 3)
        print(value // 2)
    else:
        print(value > -3)
        print(value // -2)
    # A small result does not imply that its operands have small representations.
    print(value - value)
    print(value // value)
    print(value % value)


def division() -> None:
    values = [10, 20, 30]
    for dividend in range(-3, 4):
        for divisor in range(-3, 4):
            if divisor != 0:
                print(dividend // divisor)
                print(dividend % divisor)
    position = 3
    if position >= 0:
        print(values[position // -2])
        print(values[position % -2])


branches(False)
branches(True)
loops()
aliases()
assignments()
comparisons()
copies()
ownership()
arithmetic(4611686018427387903)
arithmetic(-4611686018427387904)
arithmetic(18446744073709551616)
arithmetic(-18446744073709551616)
division()
