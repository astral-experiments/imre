class Count:
    def __init__(self, value):
        self.value = value
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return self.value


class Text(str):
    pass


def current():
    for text, n in (("ab", 3), ("", 7), ("x", 0), ("abc", -2), ("é", True)):
        print(repr(text * n), repr(n * text))
    n = Count(2)
    text = Text("abc")
    print(text * n, n * text, n.calls, type(text * 1) is str)
    for value, count in (("", 1 << 100), ("abc", 9223372036854775807), ("a", None)):
        try:
            print(value * count)
        except (TypeError, OverflowError) as error:
            print(type(error).__name__)
