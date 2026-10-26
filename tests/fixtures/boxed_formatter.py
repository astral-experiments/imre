from _string import formatter_field_name_split, formatter_parser
from string import Formatter


class Tagged(Formatter):
    def format_field(self, value, spec):
        return "[" + str(value) + "|" + spec + "]"

    @classmethod
    def owner(cls):
        return cls


class Child(Tagged):
    pass


def current():
    print(Child.owner() is Child, Child().owner() is Child, Tagged.owner.__self__ is Tagged)
    for source in ("a{{", "x{name!r:>5}終", "{0[X].attr}"):
        print(list(formatter_parser(source)))
    key, rest = formatter_field_name_split("١[02].東京")
    print(key, list(rest))

    cursor = formatter_parser("ok{{bad}")
    for part in cursor:
        print(part)
        break
    try:
        list(cursor)
    except ValueError as error:
        print(str(error))
    print(list(cursor))
    print(Formatter().format("Hello, {} {name}!", "Málaga", name="東京"))
    print(Tagged().vformat("{data[x]}{0!s:>{width}}", ("hi",), {"data": {"x": 42}, "width": 5}))
