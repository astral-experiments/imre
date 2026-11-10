import ast


def current():
    events = []

    class Unused:
        def __del__(self):
            events.append("release")

    class Fields:
        def __get__(self, instance, owner):
            events.append("names")
            return "x", Unused()

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

    match Row():
        case Row(position, key=value):
            assert (position, value) == (1, 2)
        case _:
            raise AssertionError("row did not match")

    assert events == ["names", "position", "release", "keyword"]
    assert ast.Expr.__match_args__ is ast.Expr._fields
