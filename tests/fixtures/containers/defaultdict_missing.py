from collections import defaultdict


def evaluated_key(log: list[int], fail: bool) -> int:
    log.append(9)
    if fail:
        raise ValueError("conversion did not reach the factory")
    return 9


def main() -> None:
    values: defaultdict[int, list[int]] = defaultdict(list)
    alias = values
    first_factory = alias.default_factory
    print(first_factory is not None)
    if first_factory is not None:
        print(first_factory is list)
    original = [11]
    values[2] = original
    values[3] = [12]
    returned = values.__missing__(2)
    print(returned is original)
    returned.append(13)
    print(values[2])
    print(list(values))
    fresh = defaultdict.__missing__(values, 1)
    print(fresh)
    print(fresh is values[1])
    fresh.append(8)
    print(values.__missing__(1) is fresh)
    print(values[1])
    print(list(values))
    calls: list[int] = []
    try:
        values.__missing__(evaluated_key(calls, True))
    except ValueError as error:
        print(str(error))
    print(calls)
    print(9 in values)
    alias.default_factory = None
    print(values.default_factory is None)
    try:
        values.__missing__(2)
    except KeyError as error:
        print(str(error))
    try:
        defaultdict.__missing__(values, 5)
    except KeyError as error:
        print(str(error))
    print(values[2] is original)
    print(5 in values)
    values.default_factory = list
    restored = alias.default_factory
    if restored is not None:
        print(restored is list)
    print(values.__missing__(5))
    print(5 in values)
    print(values[5] is fresh)
    base: dict[int, list[int]] = {1: [4]}
    try:
        defaultdict.__missing__(base, 1)
    except TypeError as error:
        print(str(error))
    no_factory = values.fromkeys([2], original)
    try:
        defaultdict.__missing__(no_factory, 2)
    except KeyError as error:
        print(str(error))
    print(no_factory[2] is original)
    other: defaultdict[str, list[str]] = defaultdict(list)
    text = other.__missing__("name")
    text.append("first")
    print(other["name"])
    print(text is other.__missing__("name"))
    other.default_factory = None
    try:
        other.__missing__("name")
    except KeyError as error:
        print(str(error))
    print(text is other["name"])


main()
