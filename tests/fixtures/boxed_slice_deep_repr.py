def chain(depth):
    value = 0
    for _ in range(depth):
        value = slice(value)
    return value


def current(depths=(1, 10, 5000, 12000, 13000, 16500, 40000)):
    exhausted = False
    for depth in depths:
        value = chain(depth)
        expected = "slice(None, " * depth + "0" + ", None)" * depth
        limits = []
        for name, render in (("repr", repr), ("str", str)):
            try:
                text = render(value)
            except RecursionError:
                print(depth, name, "recursion")
                limits.append(True)
            else:
                assert text == expected
                print(depth, name, len(text), text[:20], text[-20:])
                limits.append(False)
        assert limits[0] == limits[1]
        assert not exhausted or limits[0]
        exhausted = limits[0]
    value = slice([1, 2], (3,), None)
    print(repr(value), str(value))
