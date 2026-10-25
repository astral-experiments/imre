from collections import Counter, defaultdict


def eager(rows: dict[str, tuple[str, str]], log: list[int]) -> tuple[str, str]:
    log.append(len(rows))
    return ("eager", "default")


def probe(default: tuple[str, str] | None) -> None:
    rows = {"one": ("a", "x"), "two": ("b", "y")}
    log: list[int] = []
    print(rows.pop("one", eager(rows, log))[0])
    print(log[0])
    print(rows.pop("one", eager(rows, log))[0])
    print(log[1])
    print(rows.pop("one", None) is None)
    entry = rows.pop("two", default)
    if entry is not None:
        print(entry[1])
    entry = rows.pop("two", default)
    if entry is not None:
        print(entry[0])
    print(len(rows))
    try:
        rows.pop("one")
    except KeyError as e:
        print(str(e))


def main() -> None:
    probe(None)
    probe(("fallback", "kept"))
    nullable: dict[int, str | None] = {1: None}
    print(nullable.pop(1) is None)
    gone = nullable.pop(1, "gone")
    assert gone is not None
    print(gone)
    rows = {"a": [42], "b": [0]}
    try:
        for key in rows:
            keep = rows.pop(key)
            keep.append(7)
            print(keep[1])
    except RuntimeError as e:
        print(str(e))
    for key in rows:
        print(key)
    dd: defaultdict[str, list[int]] = defaultdict(list)
    fallback: list[int] = []
    retained = dd.pop("missing", fallback)
    retained.append(13)
    print(fallback[0])
    print(len(dd))
    counts: Counter[str] = Counter()
    try:
        counts.pop("absent")
    except KeyError as e:
        print(str(e))


main()
