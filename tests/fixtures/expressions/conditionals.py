"""Concrete branches retain their context, optional facts, and shared identity."""


class Row:
    words: list[str]

    def __init__(self, empty: bool) -> None:
        self.words = [] if empty else ["initial"]


def label(value: str | None) -> str:
    return value + "!" if value is not None else "missing"


def alternate_label(value: str | None) -> str:
    return "missing" if value is None else value + "?"


def nested(first: int | None, second: int | None) -> int:
    return first if first is not None else second if second is not None else -1


def words(flag: bool) -> list[str]:
    return [] if flag else ["present"]


def mapping(flag: bool) -> dict[str, list[str]]:
    return {} if flag else {"row": []}


def unique(flag: bool) -> set[str]:
    return set() if flag else {"word"}


def maybe_words(flag: bool) -> list[str] | None:
    return [] if flag else None


def maybe_number(flag: bool) -> int | None:
    return 1000000000000000000000000000000 if flag else None


def maybe_flag(flag: bool) -> bool | None:
    return False if flag else None


def maybe_row(flag: bool, row: Row) -> Row | None:
    return row if flag else None


def number_or_missing(value: int | None, flag: bool) -> int | None:
    return value if flag else 8


def add_after_conditional(value: int | None) -> int:
    result = value + 1 if value is not None else 0
    return value + result


def letters(value: str | None) -> list[str]:
    return [character for character in value] if value is not None else []


def reversed_label(value: str | None) -> str:
    return value[::-1] if value is not None else "missing"


def empty_slice() -> list[str]:
    return [][:]


def contextual_slice(flag: bool) -> list[str]:
    return ([] if flag else [character for character in "aé"])[::-1]


def nested_slice() -> list[list[str]]:
    return [[] for _index in range(2)][:]


def main() -> None:
    print(add_after_conditional(4))
    try:
        print(add_after_conditional(None))
    except TypeError as error:
        print(str(error))
    print(label(None))
    print(label("é"))
    print(alternate_label(None))
    print(alternate_label("🌍"))
    print(nested(None, None))
    print(nested(None, 7))
    print(nested(9, 7))
    print(len(letters(None)))
    print(len(letters("aé🌍")))
    print(reversed_label(None))
    print(reversed_label("aé🌍"))
    print(len(empty_slice()))
    for group in nested_slice():
        print(len(group))
    row = Row(True)
    for flag in [False, True]:
        print(10 if flag else -20)
        print(True if flag else False)  # noqa: SIM210 — exercise bool branch values
        print("left" if flag else "right")
        print(None if flag else None)  # noqa: RUF034 — exercise None branch values
        print(len(words(flag)))
        print(len(mapping(flag)))
        print(len(unique(flag)))
        for character in contextual_slice(flag):
            print(character)
        optional_words = maybe_words(flag)
        print(-1 if optional_words is None else len(optional_words))
        optional_number = maybe_number(flag)
        print("none" if optional_number is None else str(optional_number))
        optional_flag = maybe_flag(flag)
        print("none" if optional_flag is None else str(optional_flag))
        optional_row = maybe_row(flag, row)
        if optional_row is not None:
            optional_row.words.append("shared")
        print(len(row.words))
        record: tuple[int, list[str]] = (1, []) if flag else (2, ["tuple"])
        print(record[0])
        print(len(record[1]))
        nested_words: list[list[str]] = [[] if flag else ["nested"]]
        print(len(nested_words[0]))
        selected = number_or_missing(None, flag)
        print("none" if selected is None else str(selected))
        inferred_number = 1000000000000000000000000000000 if flag else None
        print("none" if inferred_number is None else str(inferred_number))
        inferred_flag = None if flag else False
        print("none" if inferred_flag is None else str(inferred_flag))
        inferred_optional = optional_number if flag else 8
        print("none" if inferred_optional is None else str(inferred_optional))
        inferred_row = None if flag else row
        print("none" if inferred_row is None else str(len(inferred_row.words)))
        selected_row = row if flag else Row(False)
        selected_row.words.append("selected")
        print(len(row.words))


main()
