from itertools import compress


def current():
    events = []
    failure = ValueError("selector failed")

    class Source:
        def __init__(self, label, values):
            self.label, self.values = label, iter(values)

        def __iter__(self):
            events.append("iter " + self.label)
            return self

        def __next__(self):
            events.append(self.label)
            return next(self.values)

    class BadSelector:
        def __bool__(self):
            raise failure

    data = Source("data", [10, 20, 30, 40, 50])
    flags = Source("flags", [False, BadSelector(), True])
    cursor = compress(data=data, selectors=flags)
    assert events == ["iter data", "iter flags"]
    assert iter(cursor) is cursor
    try:
        next(cursor)
    except ValueError as exc:
        assert exc is failure
    else:
        raise AssertionError("lost selector error")
    assert next(cursor) == 30
    assert next(cursor, None) is None
    assert next(data) == 50
    assert events[2:] == ["data", "flags"] * 4 + ["data"]

    class Child(compress):
        pass
    child = Child("abc", [1, 0, 1])
    child.data = "unrelated"
    assert type(child) is Child and list(child) == ["a", "c"]
    print("compress")


if __name__ == "__main__":
    current()
