"""Keyword bindings supply container contexts and retain optional narrowing."""


def fill(
    words: list[str],
    mapping: dict[str, int],
    unique: set[str],
    pairs: list[tuple[int, str]],
) -> str:
    words.append("word")
    mapping["entry"] = 7
    unique.add("one")
    pairs.append((8, "pair"))
    return f"{words[0]}:{mapping['entry']}:{len(unique)}:{pairs[0][0]}:{pairs[0][1]}"


def label(number: int, *, text: str = "value") -> str:
    return f"{text}:{number}"


def preserve(*, value: int | None = None) -> int | None:
    return value


def main() -> None:
    print(fill(unique=set(), pairs=[], words=[], mapping={}))
    print(fill([], unique=set(), mapping=dict(), pairs=[]))  # noqa: C408 — constructor context is under test
    number: int | None = 1000000000000000000000000000005
    if number is not None:
        print(label(text="narrowed", number=number))
    present = preserve(value=number)
    if present is not None:
        print(present)
    print(preserve() is None)
    words = [label(text=f"row{index}", number=index) for index in range(3)]
    for word in words:
        print(word)
    values: list[int | None] = [None, 4, None, 5]
    narrowed = [label(number=value) for value in values if value is not None]
    for word in narrowed:
        print(word)
    nested = [
        [label(number=inner + outer) for inner in range(outer)] for outer in range(3)
    ]
    for row in nested:
        for word in row:
            print(word)


main()
