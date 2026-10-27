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
