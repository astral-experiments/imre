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
    conversion_order()
    class Text(str):
        def __add__(self, other): raise AssertionError("unexpected addition")
        def __radd__(self, other): raise AssertionError("unexpected reflected addition")

    class Field:
        def __format__(self, spec):
            specs.append(spec)
            return result

    class Number(int):
        def __str__(self):
            return "count"

    result = Text("formatted")
    spec = Text("x")
    specs = []
    field = Field()
    assert format(field, spec) is result and specs[0] is spec
    assert f"{field}" is result and "{}".format(field) is result
    assert f"[{field}{field}]" == "[formattedformatted]"
    assert specs == ["x", "", "", "", ""]
    assert format(Number(31)) == "count" and format(Number(31), "x") == "1f"
    label = "abc"
    assert f"{label=}" == "label='abc'" and f"{label=:}" == "label=abc"

    class Word:
        def __str__(self):
            return "label"

        def __repr__(self):
            return "café"

    word = Word()
    assert "{!s}/{!r}/{!a}".format(word, word, word) == r"label/café/caf\xe9"

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
    for render in (
        lambda: f"{huge}",
        lambda: "{}".format(huge),
        lambda: format(huge, ""),
        lambda: format(huge, "d"),
        lambda: format(huge, "x"),
    ):
        try:
            print(len(render()))
        except ValueError as error:
            print(str(error))

    print("[%5.2s][%-6.3r][%.2a][%4c]" % ("éπx", "abc", "a", "x"))
    print("[%*.*s][%.*s]" % (-6, 2, "abc", -1, "hidden"))
    print("%(name)6.2s/%(name)-4.1s" % {"name": "abcd"})
    print("[%*d][%0*X][%#*.*x]" % (5, 23, 6, -42, -9, 4, 31))
    print("[%.*o][%*.*i]" % (4, 9, 6, -1, -3))
    print("[%.2f][%*.*e][%#.4G]" % (2.675, -12, 2, 1.25, 12345))
    print("%(n)+08.2F" % {"n": -0.0})
    try:
        print("%g" % (1 << 2000,))
    except OverflowError:
        print("integer too large for a float")
    for args in (("bad",), (1 << 90, 1), (2**31, 1), (2**31 - 1, 1)):
        try:
            print("%.*x" % args)
        except (TypeError, OverflowError) as error:
            print(type(error).__name__)
    for args in (("bad", "value"), (2,), (2**80, "value")):
        try:
            print("%*s" % args)
        except (TypeError, OverflowError) as error:
            print(type(error).__name__)

    class Lookup:
        def __getitem__(self, key):
            return key.upper()

    assert "%(left)s:%(right)s" % Lookup() == "LEFT:RIGHT"

    class Label:
        def __str__(self):
            return "text"

        def __repr__(self):
            return "café"

    label = Label()
    print("%s %r %a" % (label, label, label), "%5.3a" % label)


def conversion_order():
    events = []
    class Value:
        def __repr__(self):
            events.append("repr")
            return "猫"
    class Broken:
        def __repr__(self):
            events.append("raise")
            raise LookupError
    def spec():
        events.append("spec")
        return ""
    print(f"{Value()!r:{spec()}}", events)
    try:
        f"{Broken()!r:{spec()}}"
    except LookupError:
        print(events)


def integer_allocation():
    value = (1 << 8190) + 1
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        format(value, "")
    except MemoryError as error:
        print("caught", error.args)
    print(value.bit_length(), len(format(value, "")), format(value, "") == str(value))
