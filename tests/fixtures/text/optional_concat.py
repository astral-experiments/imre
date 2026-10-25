def record(label: str, value: str | None) -> str | None:
    print(label)
    return value


def add(left: str | None, right: str | None) -> None:
    try:
        print(record("left", left) + record("right", right))
    except TypeError as error:
        print(str(error))


def right_failure() -> str:
    print("right raises")
    raise ValueError("right failed")


def main() -> None:
    add("café", "!")
    add(None, "right")
    add("left", None)
    add(None, None)
    original = "a longer string"
    empty = record("empty", "")
    print((original + empty) is original)
    print((empty + original) is original)
    try:
        print(record("left None", None) + right_failure())
    except ValueError as error:
        print(str(error))


main()
