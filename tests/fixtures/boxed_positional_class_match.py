import ast


def current():
    events = []

    class Previous:
        def __init__(self, name):
            self.name = name

        def __del__(self):
            events.append(self.name)

    class Fields:
        def __get__(self, instance, owner):
            events.append("names")
            return "x", Previous("release")

    class Row:
        __match_args__ = Fields()

        @property
        def x(self):
            events.append("position")
            return 1

        @property
        def key(self):
            events.append("keyword")
            return 2

    position = Previous("old position")
    value = Previous("old value")
    match Row():
        case Row(position, key=value):
            assert (position, value) == (1, 2)
        case _:
            raise AssertionError("row did not match")

    assert events == [
        "names",
        "position",
        "release",
        "keyword",
        "old position",
        "old value",
    ]
    assert ast.Expr.__match_args__ is ast.Expr._fields
