from collections import defaultdict as groups


class Row:
    number: int

    def __init__(self, number: int) -> None:
        self.number = number


def ordinary(mapping: dict[str, list[int]]) -> None:
    print(mapping.get("base") is None)
    mapping["base"].append(12)


def insert(source: dict[str, int], log: list[str]) -> dict[str, int]:
    log.append("right")
    source["late"] = 1
    return {"other": 2}


def left(source: dict[str, int], log: list[str]) -> dict[str, int]:
    log.append("left")
    return source


def checked_key(fail: bool) -> str:
    if fail:
        raise ValueError("key failed")
    return "checked"


class FirstParty:
    def __init__(self) -> None:
        pass

    def keys(self) -> int:
        return 4


def main() -> None:
    rows: groups[str, list[Row]] = groups(list)
    try:
        rows[checked_key(True)]
    except ValueError as error:
        print(str(error))
        print(len(rows))
    print(rows.get("missing") is None)
    print("missing" in rows)
    print(len(rows.keys() | rows.keys()))
    saved = rows["missing"]
    rows["missing"].append(Row(8))
    print(saved[0].number)
    rows["distinct"].append(Row(3))
    print(len(saved))
    del rows["missing"]
    print(saved[0].number)
    print(len(rows["missing"]))
    for key in rows:
        print(key)

    numbers: groups[str, list[int]] = groups(list)
    ordinary(numbers)
    print(numbers["base"][0])
    print(numbers.get("fallback", [9])[0])
    print(len(numbers))
    match numbers:
        case {"absent": _}:
            print("unexpected key")
        case {"base": present, **rest}:
            print(present[0])
            print(len(rest))
            try:
                rest["absent"]
            except KeyError:
                print("ordinary rest")
    print("absent" in numbers)
    try:
        for key in numbers:
            numbers["another"].append(4)
    except RuntimeError as error:
        print(str(error))

    first = {"first": 0}
    log: list[str] = []
    keys = left(first, log).keys() | insert(first, log).keys() | {"last", "first"}
    first["after"] = 1
    for key in sorted(keys):
        print(key)
    print(",".join(log))
    keys.add("detached")
    print("detached" in first)
    original = {"original": 1}
    rebound = original.keys() | (original := {"replacement": 2}).keys()
    print("original" in rebound)
    print("replacement" in rebound)
    print("original" in original)
    print(len({"first", "second"} | {"first", "third"}))
    print(FirstParty().keys() | 2)

    a = {"é": 1, "": 2, "same": 3}
    b = {"same": 1, "ab": 2, "è": 3}
    c = {"ab": 4, "z": 5}
    alias = a
    print(",".join(sorted(a.keys() | (b.keys() | c.keys()) | alias.keys())))
    snapshot = sorted(a.keys() | b.keys() | insert(a, log).keys())
    print("late" in snapshot)
    print("late" in a)
    print(",".join(sorted(c.keys() | a.keys(), reverse=True)))


main()
