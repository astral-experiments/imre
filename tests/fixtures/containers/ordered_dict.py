from collections import OrderedDict


def put(source: dict[str, int]) -> None:
    source['through base'] = 6


def keys_from_ordered(source: OrderedDict[str, int]) -> OrderedDict[str, int]:
    return source.fromkeys(["b", "a", "b"], 1)


def keys_from_base(source: dict[str, int]) -> dict[str, int]:
    return source.fromkeys(["b", "a", "b"], 1)


def tuple_key(n: int) -> tuple[tuple[int, int], int]:
    return ((n, n + 1), n)


def main() -> None:
    empty: OrderedDict[str, int] = OrderedDict()
    print(empty)
    reversed_keys = OrderedDict(a=1, b=1)
    print(keys_from_ordered(empty))
    print(keys_from_ordered(empty) == reversed_keys)
    print(keys_from_base(empty) == reversed_keys)
    print(keys_from_base({"ignored": 9}) == reversed_keys)
    made = OrderedDict([('c', 1), ('b', 2), ('c', 3)], last=4)
    print(made)
    print(list(made))
    made['b'] = 7
    print(made)
    del made['c']
    made['c'] = 3
    print(made)

    fresh = OrderedDict(made)
    print(fresh is made)
    print(fresh == made)
    # The same live keys and values compare equal despite different table holes.
    fresh = OrderedDict(b=7, last=4, c=3)
    reversed_order = OrderedDict(c=3, last=4, b=7)
    print(made == fresh)
    print(made == reversed_order)
    print(made != reversed_order)
    print(made == dict(reversed_order))
    print(dict(reversed_order) == made)
    print(made == OrderedDict(b=8, last=4, c=3))
    print(made == empty)
    print(made.copy())
    put(made)
    print(made)
    made.update(last=9, tail=10)
    print(made)

    children = OrderedDict(one=[1], two=[2])
    shallow = children.copy()
    shallow['one'].append(3)
    print(children)
    print(shallow == children)
    print(shallow['one'] is children['one'])
    children.clear()
    print(children)
    print(shallow)

    first: OrderedDict[tuple[tuple[int, int], int], int] = OrderedDict()
    second: OrderedDict[tuple[tuple[int, int], int], int] = OrderedDict()
    for i in range(3):
        first[tuple_key(i)] = i
        second[tuple_key(i)] = i
    print(first == second)
    del second[tuple_key(0)]
    second[tuple_key(0)] = 0
    print(first != second)
    print(first == dict(second))

    active = OrderedDict(a=1, b=2, c=3)
    for key in active:
        active[key] = 0
        print(key)
    print(active)
    try:
        for key in active:
            del active['b']
            active['b'] = 2
    except RuntimeError as error:
        print(error)
    print(active)
    single = OrderedDict(one=1)
    for key in single:
        single['two'] = 2
        print(key)
    print(single)

    for target in ['b', 'c']:
        entries = OrderedDict(a=1, b=2)
        try:
            for key, value in entries.items():
                print(repr((key, value, entries.setdefault(target, 3))))
        except RuntimeError as error:
            print(error)
        print(list(entries))


main()
