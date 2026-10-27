class Index:
    def __index__(self):
        return -3


class FormatNumber(Index):
    def __int__(self):
        return 5


class DescriptorOwner:
    partition = str.partition


class Text(str):
    pass


class FailingFlag:
    def __bool__(self):
        raise ValueError("keepends")


def current():
    for flag in (None, [], [0], 1 << 100):
        print("a\r\nb".splitlines(keepends=flag))
    for call in (
        lambda: "x".splitlines(0, 1),
        lambda: "x".splitlines(1, keepends=False),
        lambda: "x".splitlines(invalid=1),
    ):
        try:
            call()
        except TypeError as error:
            print(error.args)
    try:
        "".splitlines(FailingFlag())
    except ValueError as error:
        print(error)
    text = "é🦊:東:🦊"
    print(["\\x%02x" % code for code in (0, 15, 31, 127, 159)])
    print("☃ %% %02d/%04d %#06X %.4o" % (7, 42, 15, 7))
    print("%(n)+06i:%(n)-5u" % {"n": 19})
    print("%+060d" % (1 << 150))
    print(str.translate("abc!", {97: None, 98: "xy", 99: 65}))
    print("abc".translate({999: []}))
    for replacement in ([], 0x110000):
        try:
            "a".translate({97: replacement})
        except (TypeError, ValueError) as error:
            print(type(error).__name__)
    number = FormatNumber()
    assert "%x %i %u %o %X %d" % ((number,) * 6) == "-3 5 5 -3 -3 5"
    print("%04x %04i %04u" % (number, number, 2.75))
    for field in ("%02d", "%02x"):
        try:
            field % "1"
        except TypeError as error:
            print(error)
    print(str.find(text, ":"), str.partition(text, ":"), str.rpartition(text, ":"))
    print(str.partition is str.partition)
    parent = super(Text, Text)
    print(parent.replace is str.replace, parent.replace("<m>", "m", "&amp;"))
    try:
        DescriptorOwner().partition(":")
    except TypeError as error:
        print(error)
    for method in (str.find, str.partition, str.rpartition, str.replace):
        try:
            method(None, ":")
        except TypeError as error:
            print(error)
    print(b"token==".rstrip(b"="))
    try:
        str.rstrip(b"token==", "=")
    except TypeError as error:
        print(error)
    for method in (text.partition, text.rpartition):
        separator = ":"
        result = method(separator)
        print(type(result) is tuple, result, result[1] is separator)
        print(method("🦊:"), method("missing"))
        try:
            method("")
        except ValueError as error:
            print(error)
    print(text.partition("missing")[0] is text, text.rpartition("missing")[2] is text)
    print("aaa".partition("aa"), "aaa".rpartition("aa"))
    for needle, start, end in (
        ("🦊", None, None), ("🦊", Index(), None), ("🦊", 1, 1), ("", 6, 6),
        ("", 7, None), (":東", -5, -2), ("🦊", -10**45, 10**45), ("", 2, 1),
    ):
        print(text.find(needle, start, end))
    print("Host: localhost".find(":"), "localhost".find(":"))
    for value in ("", " ", "\t\n", "\x1c\x1f", " x", Text("\r")):
        print(value.isspace(), str.isspace(value))
    for value in ("\0", "A", "é", "東", "🦊", b"\xff", bytearray(b"Z"), "", "é🦊", b"", bytearray(b"ab"), None, 4):
        try:
            print(ord(value))
        except TypeError as error:
            print(error)
    for value in (0, 65, 255, 0x10000, 0x10ffff, -1, 1 << 100, 65.0):
        try:
            print(ord(chr(value)))
        except (TypeError, ValueError) as error:
            print(type(error).__name__)
