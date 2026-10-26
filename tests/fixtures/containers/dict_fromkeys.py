from collections import Counter, defaultdict


def visit(key: int, events: list[int]) -> int:
    events.append(key)
    if key == 3:
        raise ValueError("bad key iterator")
    return key


def argument(keys: list[int], events: list[int]) -> int:
    events.append(20)
    keys.append(3)
    return 7


def before(keys: list[int], events: list[int]) -> list[int]:
    events.append(10)
    return keys


def base(owner: dict[str, list[int]]) -> dict[int, int]:
    return owner.fromkeys([1, 2], 9)


def counter_keywords(owner: Counter[str]) -> dict[int, int]:
    return owner.fromkeys([4], v=0)


def main() -> None:
    print(dict.fromkeys("éaé"))
    print(dict.fromkeys((4, 5), 0))
    print(dict.fromkeys(range(6), 0))
    print(dict.fromkeys(list(range(6)), 0))
    print(dict.fromkeys([9, 10, 11, 12, 13, 32], 32))
    print(dict.fromkeys({0, 1, 2, 3, 4, 5}, 0))
    keys = {4: 10, 2: 20, 7: 30}
    print(dict.fromkeys(keys, 0))
    print(keys.fromkeys([8], 0))
    print(keys)
    print({}.fromkeys([5, 3, 5]))
    empty: dict[str, int] = dict.fromkeys([], 4)
    print(empty)
    print(dict.fromkeys(()))

    shared: list[int] = []
    holders = dict.fromkeys(["b", "a", "b"], shared)
    holders["a"].append(5)
    print(holders["b"] is shared)
    print(holders["a"] is holders["b"])
    print(holders)
    print(holders is not dict.fromkeys(["b", "a"], shared))

    source = [1, 2]
    events: list[int] = []
    try:
        print(dict.fromkeys((visit(k, events) for k in before(source, events)), argument(source, events)))
    except ValueError as error:
        print(error)
    print(events)
    print(source)
    # An error from the value argument precedes the first pull.
    early_events: list[int] = []
    try:
        print(dict.fromkeys((visit(k, early_events) for k in before(source, early_events)), visit(3, early_events)))
    except ValueError as error:
        print(error)
    print(early_events)

    try:
        print(dict.fromkeys(3))
    except TypeError as error:
        print(error)
    counts = Counter("aba")
    try:
        print(counts.fromkeys([4], 0))
    except NotImplementedError as error:
        print(error)
    try:
        counter_keywords(counts)
    except NotImplementedError as error:
        print(error)
    keyword_events: list[int] = []
    try:
        counts.fromkeys(
            v=visit(1, keyword_events),
            iterable=(visit(3, keyword_events) for _ in before(source, keyword_events)),
        )
    except NotImplementedError as error:
        print(error)
    print(keyword_events)
    groups: defaultdict[str, list[int]] = defaultdict(list)
    groups['prior'].append(5)
    new = base(groups)
    print(new)
    try:
        print(new[3])
    except KeyError as error:
        print(error)
    print(groups)


main()
