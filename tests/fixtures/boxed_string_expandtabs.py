class Width:
    def __init__(self, n):
        self.n = n
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return self.n


class Text(str):
    pass


def current():
    source = "abc\r\tX\nA\tb"
    print(repr(source.expandtabs()), repr(source.expandtabs(tabsize=4)))
    print(repr("a\tb".expandtabs(-1)), repr("\t".expandtabs(True)))
    assert "a\tb".expandtabs(Width(True)) == "a b"
    assert "a\tb".expandtabs(Width(False)) == "ab"
    plain = "plain text"
    width = Width(3)
    print(plain.expandtabs(width) is plain, width.calls)
    print(type(Text(plain).expandtabs()) is str)
    for value in (1 << 32, -(1 << 40), None, Width("bad")):
        try:
            plain.expandtabs(value)
        except (TypeError, OverflowError) as error:
            print(type(error).__name__)
    try:
        plain.expandtabs(2, tabsize=3)
    except TypeError as error:
        assert error.args == ("expandtabs() takes at most 1 argument (2 given)",)
    else:
        raise AssertionError("expected a type error")

    width = Width(4)
    assert str.zfill("-é", width) == "-00é" and width.calls == 1
    assert "+\ud800".zfill(4) == "+00\ud800"
    saved = Text(plain).zfill
    first = saved(0)
    assert type(first) is str and first == plain and first is not saved(0)
    assert plain.zfill(0) is plain

    width = Width(5)
    assert str.center.__get__(Text("éx"))(width, "*") == "**éx*" and width.calls == 1
    assert "\ud800".ljust(3, "𝄞") == "\ud800𝄞𝄞"
    assert "a".rjust(2, "\udfff") == "\udfffa"
    assert plain.center(0) is plain
    try:
        plain.ljust(0, "xx")
    except TypeError:
        pass
    else:
        raise AssertionError("expected a single fill character")

    class Overridden(Text):
        def zfill(self, width):
            return "override"

    overridden = Overridden("-x")
    assert overridden.zfill(4) == "override"
    assert str.zfill.__get__(overridden)(4) == "-00x"
    assert Text.zfill is str.zfill and str.zfill.__objclass__ is str
    assert str.zfill.__text_signature__ == "($self, width, /)"
    for args in ((), (1, 2), (None,), (Width("bad"),)):
        try:
            plain.zfill(*args)
        except TypeError:
            pass
        else:
            raise AssertionError(args)
