print("library initialized")
CHARSET = "abcé"
TABLE = {value: index for index, value in enumerate(CHARSET)}
ALIAS = TABLE
ROWS = [[1, 2], [3]]


def lookup(key: str) -> int:
    return TABLE[key]


def values() -> list[int]:
    return [TABLE[key] for key in CHARSET]


def mutate() -> None:
    TABLE["a"] = 70
    ROWS[0].append(4)


def shrink() -> None:
    ROWS[0].pop()


def read_after_mutation() -> int:
    held = ROWS[0]
    index = len(held) - 1
    shrink()
    return held[index]
