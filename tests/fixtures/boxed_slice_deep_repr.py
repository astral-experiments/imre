def chain(depth):
    value = 0
    for _ in range(depth): value = slice(value)
    return value

def current():
    for depth in (1, 10, 5000, 12000, 15500, 40000):
        value = chain(depth)
        for name, fn in (("repr", repr), ("str", str)):
            try:
                text = fn(value)
                print(depth, name, len(text), text[:20], text[-20:])
            except RecursionError:
                print(depth, name, 'recursion')
    value = slice([1,2], (3,), None)
    print(repr(value), str(value))
