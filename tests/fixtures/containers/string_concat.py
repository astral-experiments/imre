"""Concatenation preserves ordered evaluation, snapshots, and exception cleanup."""


class Box:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


class Owner:
    box: Box

    def __init__(self, box: Box) -> None:
        self.box = box


class NumberBox:
    number: int

    def __init__(self, number: int) -> None:
        self.number = number


def event(log: list[int], number: int, text: str) -> str:
    log.append(number)
    return text


def fail(log: list[int], number: int) -> str:
    log.append(number)
    raise ValueError("late" + " failure")


def replace(box: Box, values: list[str], log: list[int]) -> str:
    log.append(10)
    box.text = "new field"
    values[0] = "new element"
    return "|replaced|"


def remove(values: list[str]) -> str:
    values.pop()
    return "|removed|"


def replace_owner(owner: Owner) -> str:
    owner.box = Box("new owner")
    return "|owner replaced|"


def nested(log: list[int]) -> str:
    return event(log, 6, "nested") + ":" + event(log, 7, "value")


def returned(value: str) -> str:
    return "<" + value + ">"


def replace_number(box: NumberBox) -> str:
    box.number = 0
    return ":replaced"


def remove_number(values: list[int]) -> str:
    values.pop()
    return ":removed"


def main() -> None:
    log: list[int] = []
    print(
        (event(log, 1, "a") + event(log, 2, "b"))
        + (event(log, 3, "c") + event(log, 4, "d"))
    )
    print(event(log, 5, "outer:") + nested(log) + ":end")
    print(event(log, 8, "right:") + (event(log, 9, "nested") + ("" + ":tail")))

    # A later call replaces the only external owner of an earlier field/element.
    box = Box("captured" + " field")
    values = ["captured" + " element"]
    print(box.text + values[0] + replace(box, values, log) + box.text)
    print(values[0] + remove(values) + "end")
    owner = Owner(Box("nested" + " field"))
    print(owner.box.text + replace_owner(owner) + "end")
    print(owner.box.text)

    # The inner concat is another expression in the same generated function.
    print(
        event(log, 11, "length:")
        + str(len(event(log, 12, "λ") + event(log, 13, "🙂")))
        + event(log, 14, ":done")
    )
    for number in log:
        print(number)

    print("" + "")
    print("λ\0" + "🙂" + "" + "\n尾")
    big = 1000000000000000000000000000000000000000000000000000000000007
    print(
        str(-big)
        + ":"
        + str(big)
        + ":"
        + str(0)
        + str(True)
        + str(False)
        + str(None)
        + str(ValueError("message"))
        + str(KeyError("quoted"))
    )
    # Small integer formatting uses per-leaf scratch; BigInts retain one owned
    # formatted string even after a later call removes the source's last root.
    boundaries = [
        -4611686018427387905,
        -4611686018427387904,
        -1,
        0,
        1,
        4611686018427387903,
        4611686018427387904,
        -9223372036854775808,
        9223372036854775807,
    ]
    for number in boundaries:
        print(str(number) + ":" + str(-number))
        print(str(number) + str(number))
    numbered = NumberBox(1000000000000000000000000000000000000000000000000000000000013)
    print(str(numbered.number) + replace_number(numbered) + ":end")
    numbers = [1000000000000000000000000000000000000000000000000000000000017]
    print(str(numbers[0]) + remove_number(numbers) + ":end")
    print(len(numbers))
    value = returned("alias")
    saved = [value]
    value = value + value
    print(saved[0])
    print(value)
    print(returned(value))

    failures: list[int] = []
    for index in range(40):
        try:
            print(
                event(failures, 20, str(big))
                + "prefix"
                + fail(failures, 21)
                + event(failures, 99, "must not run")
            )
        except ValueError as error:
            if index == 0:
                print(str(error))
        try:
            print(str(index) + ":" + str(big) + fail(failures, 23))
        except ValueError:
            pass
    try:
        print(fail(failures, 22) + event(failures, 99, "must not run") + "end")
    except ValueError:
        print("early failure")
    print(len(failures))
    print(failures[0])
    print(failures[1])
    print(failures[-1])


main()
