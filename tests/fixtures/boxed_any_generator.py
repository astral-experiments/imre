def generator_slots():
    marker = object()
    def values():
        yield marker
        return marker
    cursor = values()
    step = cursor.__next__
    slot = type(cursor).__next__
    assert cursor.__iter__() is cursor
    assert slot.__objclass__ is type(cursor)
    assert slot.__get__(cursor)() is marker
    try:
        step()
    except StopIteration as error:
        assert error.value is marker
    else:
        raise AssertionError("generator lost its return value")
    try:
        step()
    except StopIteration as error:
        assert error.args == ()
    else:
        raise AssertionError("exhausted generator produced a value")
    try:
        slot(iter([marker]))
    except TypeError as error:
        print(error)
    else:
        raise AssertionError("generator descriptor accepted another iterator")


def current():
    generator_slots()
    seen = []

    def observe(value):
        seen.append(value)
        return value

    items = [0, 1, 2]
    value = "outer"
    gen = (observe(value) for value in items if value != 1)
    assert not seen and iter(gen) is gen
    for it in [gen, iter(items), iter((0, 1)), iter(b"a"), iter(bytearray(b"a")), iter(range(3))]:
        print(type(it).__name__, isinstance(it, type(it)), type(type(it)) is type)
    items.append(3)
    assert any(gen)
    assert seen == [0, 2] and value == "outer"
    print(next(gen), next(gen, "done"), next(gen, "done"))
    cursor = iter(items)
    assert list(cursor) == items
    items.append(4)
    assert next(cursor, "closed") == "closed"
    buffer = bytearray(b"\x00\x00\xff")
    cursor = iter(buffer)
    assert next(cursor) == 0
    buffer[1] = 3
    assert any(byte for byte in cursor) and next(cursor) == 255
    assert not any(()) and any([False, True])
    assert any(range(10**12)) and not any(range(0))
    assert list(iter(range(5, -1, -2))) == [5, 3, 1]
    print(list(x for x in [None, False]))

    def shadow(iterator):
        return next(iterator)

    print(shadow(x * 3 for x in [2, 4]))

    def failing(x):
        raise StopIteration("element")

    gen = (failing(x) for x in [1, 2])
    try:
        next(gen)
    except RuntimeError as error:
        print(str(error))
    print(next(gen, "closed"))

    class Cursor:
        def __init__(self):
            self.values = iter([0, 1, 2])

        def __iter__(self):
            return self

        def __next__(self):
            return next(self.values)

    cursor = Cursor()
    assert iter(cursor) is cursor and any(cursor)
    assert next(cursor) == 2 and list(cursor) == []
    print(next(cursor, "done"))

    values = [0, 1, 2, None]
    cursor = iter(lambda: values.pop(0), None)
    gen = (item for item in cursor)
    assert any(gen) and next(cursor) == 2 and list(gen) == []

    class Finished(StopIteration):
        pass

    def stop_in_body(item):
        raise Finished(item)

    gen = (stop_in_body(item) for item in [1])
    try:
        next(gen)
    except RuntimeError as error:
        print(str(error))
    assert next(gen, "closed") == "closed"

    pairs = (pair for pair in [("first", 1), ("bad",), ("later", 2)])
    copied = {}
    try:
        copied.update(pairs)
    except ValueError:
        pass
    assert copied == {"first": 1} and list(pairs) == [("later", 2)]

    class Owner:
        def classes(self, source):
            return (__class__ for item in source)

        def receivers(self, source):
            return (super() for item in source)

    assert next(Owner().classes(items)) is Owner
    try:
        next(Owner().receivers(items))
    except TypeError as error:
        print(str(error))

    captures = list(lambda: value for value in [3, 7])
    print(captures[0](), captures[1]())
