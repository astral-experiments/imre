def current():
    print(sum([0.98 * (10 / 11), 0.01 * True, 0.01 * True]))
    print(sum(triple[-1] for triple in [(0, 0, 5), (6, 5, 0)]))
    print(sum([0.1] * 10), sum([1.0, 1e100, 1.0, -1e100]))
    print(sum([1.0, 10**100, 1.0, -(10**100)]))
    print(sum((n / 2 for n in range(10)), start=1000.25))
    print(sum([1, 0.5]), sum([0.5, 1]), sum([True, 1.5], 6))
    print(sum([1, 2], 1 << 90), sum([], False) is False)

    integers = iter([10, True, -2, 8])
    assert next(integers) == 10
    assert sum(integers, 5) == 12
    assert list(integers) == []
    assert sum([2**63 - 1, 1, -(2**63)]) == 0
    print(sum([-0.0]), sum([-0.0], -0.0), sum([], -0.0))
    print(sum([1e308, 1e308]), sum([float('inf'), float('-inf')]))
    try:
        sum([1.0, 10**1000])
    except OverflowError as error:
        print(str(error))

    class Stop:
        def __iter__(self):
            yield 1.25
            raise ValueError('iterator')

    try:
        sum(Stop())
    except ValueError as error:
        print(str(error))

    for values in ([1, True, 3, 4], (1, True, 3, 4), (2**63, -1, 1), b"abc"):
        reverse = reversed(values)
        print(next(reverse), sum(reverse, 5), list(reverse))
    remaining = iter(bytearray([1, 250, 3]))
    print(next(remaining), sum(remaining, 2**63 - 100), list(remaining))

    class Extra:
        def __radd__(self, value):
            return ('rounded', value)

    print(sum([0.1] * 10 + [Extra()]))
