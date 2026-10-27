from _string import formatter_field_name_split, formatter_parser
from string import Formatter


class Tagged(Formatter):
    def format_field(self, value, spec):
        return "[" + str(value) + "|" + spec + "]"

    @classmethod
    def owner(cls):
        return cls


class Child(Tagged):
    native = classmethod(len)


def current():
    wrapper = classmethod(len)
    assert wrapper.__func__ is len and wrapper.__wrapped__ is len
    assert type(Child.native) is type(Child.owner)
    assert Child.native.__func__ is len and Child.native.__self__ is Child
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
    for value in (0, -15, 2**130 + 15, True):
        print(f"{value:02X}", "{:+#08x}".format(value), format(value, "◇^9X"))
        print(format(value, "#014_X"), format(value, "+#08b"))
        for spec in ("02Q", "dX", ".2X", "+c"):
            try:
                format(value, spec)
            except ValueError as error:
                print(str(error))
    for value in (-1, 0x110000, 1 << 90):
        try:
            format(value, "2c")
        except OverflowError as error:
            print(str(error))
    huge = 1 << 14500
    for spec in ("", "d", "x"):
        try:
            print(len(format(huge, spec)))
        except ValueError as error:
            print(str(error))
