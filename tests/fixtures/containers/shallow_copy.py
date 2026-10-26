import copy
from copy import copy as duplicate
from collections import Counter, defaultdict
from http import HTTPStatus

def build(events: list[int]) -> list[list[int]]:
    events.append(1)
    return [events, events]

def through_dict(source: dict[str, list[int]]) -> dict[str, list[int]]:
    return duplicate(x=source)

def main() -> None:
    events: list[int] = []
    rows = duplicate(build(events))
    rows[0].append(2)
    print(rows[0] is rows[1])
    print(rows[0] is events)
    print(len(events))
    rows.append([9])
    print(len(rows))
    print(repr(events))
    original: dict[str, list[int]] = {"first": events, "second": events}
    cloned = duplicate(original)
    cloned["first"].append(3)
    del cloned["first"]
    cloned["third"] = [4]
    print(original is cloned)
    print(cloned["second"] is original["first"])
    print(repr(original))
    print(repr(cloned))
    keys = {"one", "two"}
    detached = copy.copy(keys)
    detached.add("three")
    print(detached is keys)
    print(len(keys))
    print(len(detached))
    c: Counter[str] = Counter("aba")
    counts = copy.copy(c)
    counts["a"] += 5
    print(c["a"])
    print(counts["a"])
    print(counts["unseen"])
    print(len(counts))
    values: defaultdict[str, list[int]] = defaultdict(list)
    values["existing"].append(3)
    m = through_dict(values)
    m["existing"].append(4)
    m["missing"].append(5)
    print(repr(values["existing"]))
    print(repr(m["missing"]))
    print("missing" in values)
    fixed = (events, "x")
    t = duplicate(fixed)
    print(t[0] is events)
    print(copy.copy(1 << 100))
    print(copy.copy("abc"))
    print(copy.copy(b"xy"))
    print(copy.copy(None) is None)
    assert copy.copy(HTTPStatus.OK) is HTTPStatus.OK

main()
