class Finished(StopIteration):
    pass


class Cursor:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls >= 3:
            raise Finished("end")
        return self.calls


class Pairs(Cursor):
    def __next__(self):
        key = Cursor.__next__(self)
        return key, key + 2


class Rows:
    def __init__(self, values):
        self.values = values

    def __iter__(self):
        return iter(self.values)


class Invalid:
    def __iter__(self):
        return []


class Broken(Cursor):
    def __next__(self):
        raise ValueError("broken")


class FailedIterable:
    def __iter__(self):
        raise TypeError("from iter")


class FailedCursor:
    def __iter__(self):
        return self

    def __next__(self):
        raise TypeError("from next")


def current():
    from functools import partial

    chain = iter([7])
    for _ in range(16384):
        chain = iter(partial(next, chain), None)
    print(next(chain), next(chain, "exhausted"))

    class RowsError(ValueError):
        def __iter__(self):
            return iter((3, 5))

    class CursorError(LookupError):
        def __iter__(self):
            return self

        def __next__(self):
            raise StopIteration

    assert list(RowsError()) == [3, 5]
    error_cursor = CursorError()
    assert iter(error_cursor) is error_cursor and next(error_cursor, 7) == 7
    assert [item for item in error_cursor] == []

    log = []
    source = [0, 2, 3]
    def select(item):
        log.append(item)
        return item
    cursor = filter(select, source)
    assert iter(cursor) is cursor and not log
    assert next(cursor) == 2 and log == [0, 2]
    source.append(4)
    assert list(cursor) == [3, 4] and log == [0, 2, 3, 4]
    assert list(filter(None, [False, "x", 0, 1])) == ["x", 1]
    from operator import length_hint

    class Text(str):
        pass
    class Data(bytes):
        pass
    assert list(reversed(Text("🌓a"))) == ["a", "🌓"]
    assert list(reversed(Data(bytes([0, 97])))) == [97, 0]

    for source, last, rest in (("é中x", "x", ["中", "é"]), (b"\x00\xffA", 65, [255, 0])):
        cursor = reversed(source)
        assert type(cursor) is reversed and iter(cursor) is cursor
        assert length_hint(cursor) == 3 and next(cursor) == last
        assert length_hint(cursor) == 2 and list(cursor) == rest
        assert length_hint(cursor) == 0 and next(cursor, None) is None

    values = iter([1, 0, 7])
    print(all([]), all(values), next(values))
    for call in (lambda: all(), lambda: any([], []), lambda: all(iterable=[])):
        try:
            call()
        except TypeError as error:
            print(error.args)


    for source in (FailedIterable(), FailedCursor()):
        try:
            print(1 in source)
        except TypeError as error:
            print(error.args)
    cursor = Cursor()
    cursor.__iter__ = lambda: []
    print(iter(cursor) is cursor)
    target = [7]
    alias = target
    target.extend(len(alias) for _ in range(3))
    try:
        target.extend(12 // value for value in (3, 2, 0))
    except ZeroDivisionError:
        print(alias)
    try:
        target += (12 // value for value in (3, 2, 0))
    except ZeroDivisionError:
        print(target is alias, alias)
    else:
        raise AssertionError("lost in-place iterator exception")
    target.extend(alias)
    print(alias)

    class Hint:
        def __iter__(self):
            events.append("iter")
            return iter((3,))

        def __length_hint__(self):
            events.append("hint")
            alias.append(9)
            raise ValueError("stop")

    events = []
    try:
        target.extend(Hint())
    except ValueError as error:
        print(str(error), events, alias)
    else:
        raise AssertionError("ignored length hint")
    growing = [4, 5]
    for index, item in enumerate(growing, start=1 << 70):
        print(index, item)
        if item == 4:
            growing.append(6)
    for item in cursor:
        print(item)
        break
    print(cursor.calls, next(cursor))
    try:
        next(cursor)
    except Finished as exc:
        print(str(exc))
    print(next(cursor, None), cursor.calls)
    print(list(Cursor()), [item for item in Cursor()])
    print(list(zip(Cursor(), range(2), strict=True)))
    try:
        list(zip(range(3), Cursor(), strict=True))
    except ValueError as exc:
        print(str(exc))
    print(dict(Pairs()))
    print(sum(Cursor(), start=1 << 90))
    print(sum(value > 0 for value in Cursor()))
    print(sum((), -3))
    try:
        sum(Broken())
    except ValueError as exc:
        print(str(exc))
    values = ["Content-Type"]
    rows = Rows(values)
    it = iter(rows)
    print(iter(it) is it, next(it))
    values.append("Content-Length")
    print(list(it))
    for value in (Invalid(), Broken()):
        try:
            list(value)
        except (TypeError, ValueError) as exc:
            print(str(exc))
    try:
        next([], "default")
    except TypeError as exc:
        print(str(exc))

    values = [None, 4, 5, 6]
    it = iter(lambda: values.pop(0), 5)
    print(iter(it) is it, list(it), next(it, "done"), values)

    def recursive():
        if recursive.entered:
            return 0
        recursive.entered = True
        list(recursive.iterator)
        for i in range(4096):
            temporary = [i, i + 1]
        return 1

    recursive.entered = False
    from functools import partial
    recursive.iterator = iter(partial(next, iter(recursive, 0)), 0)
    print(next(recursive.iterator, "exhausted"))

    for function in (iter, next):
        for args, kwargs in (((), {}), ((0, 1, 2), {}), ((), {"x": 1})):
            try:
                function(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))

    cursor = Cursor()
    print(1 in cursor, cursor.calls, next(cursor, None))
    try:
        1 in Broken()
    except ValueError as exc:
        print(str(exc))

    calls = []

    class Candidate:
        def __eq__(self, other):
            calls.append(other)
            return other == 17

    item = Candidate()
    print(17 in [item], 17 in Rows([item]), item in [item], calls)
