from operator import length_hint


def current():
    for seq in (range(5), range(1, 10, 3), range(8, -1, -3), range(0), range(3, 7, -2)):
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
    cursor = range(edge).__reversed__()
    print(cursor.__iter__() is cursor, cursor.__length_hint__())
    print(cursor.__next__(), cursor.__length_hint__())
    try:
        length_hint(cursor)
    except OverflowError as error:
        print(str(error))
