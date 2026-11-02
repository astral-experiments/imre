from itertools import count, repeat


class Label:
    def __repr__(self):
        return "label"


class Broken:
    def __repr__(self):
        raise LookupError("component repr")


class Advancing:
    def __repr__(self):
        next(self.cursor)
        return "advanced"


class Number:
    def __int__(self):
        raise AssertionError("unexpected coercion")

    def __repr__(self):
        return "number"

    def __add__(self, step):
        return self.cursor


class LabeledInt(int):
    def __repr__(self):
        return "labeled-int"


class Once(repeat):
    pass


class Override(repeat):
    def __repr__(self):
        return "overridden"


def current():
    r = Once(Label(), 1)
    assert repr(r) == str(r) == repeat.__repr__(r) == "Once(label, 1)"
    r = Override(Label())
    assert repr(r) == "overridden" and repeat.__repr__(r) == "Override(label)"
    assert repr([r]) == "[overridden]"

    element = Advancing()
    element.cursor = repeat(element, 2)
    assert repr(element.cursor) == "repeat(advanced, 2)"
    assert repeat.__repr__(element.cursor) == "repeat(advanced, 1)"
    assert next(element.cursor, None) is None
    for operation in (repr, repeat.__repr__):
        try:
            operation(repeat(Broken()))
        except LookupError as error:
            assert error.args == ("component repr",)
        else:
            raise AssertionError("swallowed component exception")

    items = []
    repeated = repeat(items, 2)
    items.append(repeated)
    assert repr(repeated) == "repeat([repeat([...], 2)], 2)"
    assert repr(items) == "[repeat([...], 2)]"

    n = Number()
    assert count.__repr__(count(n, LabeledInt(2))) == "count(number, labeled-int)"
    assert repr(count(n, LabeledInt(1))) == "count(number)"
    assert repr(count(LabeledInt(7), LabeledInt(1))) == "count(7)"
    assert repr(count(LabeledInt(1 << 90))) == "count(labeled-int)"

    n.cursor = count(n)
    assert next(n.cursor) is n
    try:
        repr(n.cursor)
    except RecursionError:
        pass
    else:
        raise AssertionError("unbounded recursive count")
    print("cursor repr")
