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


def current():
    values = iter([1, 0, 7])
    print(all([]), all(values), next(values))

    cursor = Cursor()
    cursor.__iter__ = lambda: []
    print(iter(cursor) is cursor)
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
        return 1

    recursive.entered = False
    recursive.iterator = iter(recursive, 0)
    print(next(recursive.iterator, "exhausted"))

    for function in (iter, next):
        for args, kwargs in (((), {}), ((0, 1, 2), {}), ((), {"x": 1})):
            try:
                function(*args, **kwargs)
            except TypeError as exc:
                print(str(exc))
