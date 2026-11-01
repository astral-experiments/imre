import textwrap


class Wrapper(textwrap.TextWrapper):
    def __init__(self, **options):
        print(sorted(options))
        super().__init__(**options)


class RepeatedWidth:
    def keys(self):
        return ["width"]

    def __getitem__(self, key):
        raise AssertionError("duplicate keys must be rejected before fetching their values")


def current():
    original = textwrap.TextWrapper
    textwrap.TextWrapper = Wrapper
    try:
        print(textwrap.wrap("abc def", 7, initial_indent="_"))
        print(textwrap.fill("abc def", 3))
        print(textwrap.shorten("abc def", 7))
        for call in (
            lambda: textwrap.wrap("abc", 3, width=4),
            lambda: textwrap.wrap("abc", unknown=True),
            lambda: textwrap.wrap("abc", expand_tab=True),
            lambda: textwrap.shorten("abc", 7, max_lines=2),
            lambda: textwrap.wrap("abc", width=3, **RepeatedWidth()),
            lambda: textwrap.wrap("abc", **{"width": 3}, width=4, placeholder=print("evaluated")),
        ):
            try:
                call()
            except TypeError as exc:
                print(str(exc))
            else:
                raise AssertionError("expected TypeError")
    finally:
        textwrap.TextWrapper = original
