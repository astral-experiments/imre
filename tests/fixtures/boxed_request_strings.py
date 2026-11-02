from enum import StrEnum


class Digit(StrEnum):
    TWO = "²"


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


class CustomPredicates(str):
    def isalpha(self):
        return "custom alpha"

    def isdigit(self):
        return "custom digit"


class CustomAlnum(str):
    def isalnum(self):
        return "override"


class CustomAscii(str):
    def isascii(self):
        return "override"


class CustomTitle(str):
    def istitle(self):
        return "override"


class CustomUpper(str):
    def isupper(self):
        return "override"


class FailingFlag:
    def __bool__(self):
        raise ValueError("keepends")


def current():
    check_index()
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
    assert "%(n)d %(n)i %(n)u %(n)o %(n)x %(n)X" % {"n": number} == "5 5 5 -3 -3 -3"
    fields = {"name": "Imre", "count": 3, "ratio": 2.5}
    assert "%(name)s/%(count)d/%(ratio).1f" % fields == "Imre/3/2.5"
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
    assert parent.replace(Text("aaa"), "a", "x", count=Index()) == "xxx"
    try:
        parent.replace("abc", None, "", count=Index())
    except TypeError as error:
        assert str(error) == "replace() argument 1 must be str, not None"
    else:
        raise AssertionError("str.replace accepted None as its old string")
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
    bound_title = Digit.TWO.istitle
    print(bound_title(), str.istitle(Digit.TWO))
    bound_digit = Digit.TWO.isdigit
    print(bound_digit(), str.isalpha(Digit.TWO))
    for value in (Text("é"), Text("²"), "\ud800", Text("7\udfff"), Text("a\udfff"),
                  CustomPredicates("a"), CustomPredicates("1")):
        bound_alpha = value.isalpha
        bound_digit = value.isdigit
        print(bound_alpha(), str.isalpha(value), bound_digit(), str.isdigit(value))
    for call in (lambda: str.isalpha(), lambda: str.isdigit(None), lambda: "a".isalpha(0),
                 lambda: "2".isdigit(unexpected=0)):
        try:
            call()
        except TypeError as error:
            print(error)
    print(text.partition("missing")[0] is text, text.rpartition("missing")[2] is text)
    print("aaa".partition("aa"), "aaa".rpartition("aa"))
    for needle, start, end in (
        ("🦊", None, None), ("🦊", Index(), None), ("🦊", 1, 1), ("", 6, 6),
        ("", 7, None), (":東", -5, -2), ("🦊", -10**45, 10**45), ("", 2, 1),
    ):
        print(text.find(needle, start, end), text.count(needle, start, end))
    print("Host: localhost".find(":"), "localhost".find(":"))
    for value in ("", " ", "\t\n", "\x1c\x1f", " x", Text("\r")):
        print(value.isspace(), str.isspace(value))
    for value in ("", "0Ab", "aa b", "²", "𑁦", "𝟜", "🦊", Text("東"), CustomAlnum("!")):
        print(value.isalnum(), str.isalnum(value))
    for call in (lambda: str.isalnum(), lambda: str.isalnum(1), lambda: "a".isalnum(1),
                 lambda: "a".isalnum(value="x")):
        try:
            call()
        except TypeError as error:
            print(error)
    for value in ("", "\0\x7f", "A\x80", "🦊", "\ud800", Text("A\udfff"), CustomAscii("a")):
        saved = value.isascii
        print(saved(), str.isascii(value))
    for call in (lambda: str.isascii(), lambda: str.isascii(1), lambda: "a".isascii(1),
                 lambda: "a".isascii(wrong=1)):
        try:
            call()
        except TypeError as error:
            print(error)
    for value in ("", "ǅa", "A\u0345", "\ud800", "A\udc00B", "A\udc00b", CustomTitle("ABC")):
        saved = value.istitle
        print(saved(), str.istitle(value))
    for call in (lambda: "A".istitle(1), lambda: "A".istitle(value=1)):
        try:
            call()
        except TypeError as error:
            print(error)
    for value in ("", "7_東", "ʕ_東", "aǅ", "ⅷ", Text("𐐨"), "\ud800a"):
        saved = value.islower
        print(value.islower(), saved(), str.islower(value))
    for value in ("", "7_東", "É_7東", "Éa", "Aǅ", "Ⅰ", "Aʕ", Text("𐐀"), CustomUpper("a")):
        print(value.isupper(), str.isupper(value))
    for call in (lambda: str.isupper(), lambda: str.isupper(1), lambda: "A".isupper(1),
                 lambda: "A".isupper(value="x")):
        try:
            call()
        except TypeError as error:
            print(error)
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


def check_index():
    class Bound:
        def __init__(self, n):
            self.n = n

        def __index__(self):
            calls.append(self.n)
            return self.n

    calls = []
    text = Text("é🦊:東:🦊")
    assert text.index(Text("🦊"), Bound(2), Bound(6)) == 5
    assert calls == [2, 6]
    assert str.index(text, ":", -10**80, 10**80) == 2
    assert str.rindex(text, ":", -10**80, 10**80) == 4
    surrogate = Text("a\ud800🦊\udfff")
    assert surrogate.index(Text("\ud800🦊"), None, -1) == 1
    assert str.index(surrogate, "🦊") == 2
    assert surrogate.index("", 4, 10**80) == 4
    repeated = Text("a\ud800\ud800\ud800")
    assert repeated.find(Text("\ud800\ud800")) == 1
    assert repeated.rfind(Text("\ud800\ud800"), 1, 10**80) == 2
    assert repeated.rindex(Text("\ud800\ud800"), 1, 10**80) == 2
    assert repeated.rfind("", 1, -1) == 3
    assert repeated.rfind("", 5) == -1
    assert "abc".find("\ud800") == "abc".rfind("\ud800") == -1
    for call in (
        lambda: surrogate.index("", 5),
        lambda: surrogate.index("", 2, 1),
        lambda: surrogate.index("🦊", 3),
        lambda: "abc".index("\udfff"),
        lambda: repeated.rindex("", 5),
    ):
        try:
            call()
        except ValueError as error:
            assert str(error) == "substring not found"
        else:
            raise AssertionError("expected missing substring")
    for call, message in (
        (lambda: text.index(), "index expected at least 1 argument, got 0"),
        (lambda: text.index("x", 0, 1, 2), "index expected at most 3 arguments, got 4"),
        (lambda: text.index(sub="x"), "str.index() takes no keyword arguments"),
        (lambda: text.index(1, Bound(0)), "index() argument 1 must be str, not int"),
        (lambda: str.index(1, "x"), "descriptor 'index' for 'str' objects doesn't apply to a 'int' object"),
    ):
        try:
            call()
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("expected invalid index arguments")
    assert calls == [2, 6]
