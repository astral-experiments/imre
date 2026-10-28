from operator import length_hint


def current():
    for seq in (range(5), range(1, 10, 3), range(8, -1, -3), range(0), range(3, 7, -2)):
        print(seq.start, seq.stop, seq.step)
        left = reversed(seq)
        right = reversed(seq)
        print(iter(left) is left, left is right, length_hint(left))
        print(next(left, None), list(right), list(left), next(left, None), next(left, None))

    # Take a bounded prefix even when the length and endpoints exceed Py_ssize_t.
    edge = 1 << 100
    for seq in (range(-edge, edge, 3), range(edge, -edge, -7), range(5, edge, edge // 3)):
        left = reversed(seq)
        print(type(left).__name__, next(left), next(left), next(left))
    print(list(reversed(range(edge + 2, edge + 9, 3))))
    print(list(reversed(range(-edge, -edge - 10, -4))))
    print(list(reversed(range(edge, -edge, edge))))
    for left, right in ((range(0), range(4, 1)),
                        (range(2, 3), range(2, 8, 7)),
                        (range(edge, 0, -3), range(edge, 1, -3)),
                        (range(0, edge, 2), range(0, edge - 1, 2)),
                        (range(0, 4), range(0, 8, 2))):
        print(left == right, left != right, {left: 1}.get(right), len({left, right}))
    cursor = range(edge).__reversed__()
    sliced = range(0, edge, 3)[::-2]
    print(sliced.start, sliced.stop, sliced.step)
    try:
        sliced.step = 1
    except AttributeError as error:
        print(str(error))
    print(cursor.__iter__() is cursor, cursor.__length_hint__())
    print(cursor.__next__(), cursor.__length_hint__())
    try:
        length_hint(cursor)
    except OverflowError as error:
        print(str(error))

    check_receivers(range)

    for needle in (True, 4.0, 3.5):
        seq = range(0, 7, 2)
        print(seq.count(needle), seq.index(needle) if needle in seq else None)
    huge = range(-edge, edge, 2)
    print(edge - 2 in huge, edge in huge, huge.count(1), huge.index(edge - 2))

    seq = range(1, 8, 2)
    print(repr([seq]), seq.__repr__(), range.__len__(seq), seq.__getitem__(-1))
    print(seq.__eq__(range(1, 9, 2)), seq.__ne__(()) is NotImplemented,
          seq.__lt__(seq) is NotImplemented, seq.__hash__() == hash(seq))


def check_receivers(native_range):
    class range:
        pass

    class range_iterator:
        pass

    for method, obj in ((native_range.__reversed__, range()),
                        (type(iter(native_range(2))).__length_hint__, range_iterator())):
        try:
            method(obj)
        except TypeError:
            print("rejected")
