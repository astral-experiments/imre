class Word(str):
    pass


class Wrapped(Word):
    def __new__(cls, text):
        return str.__new__(cls, "<" + text + ">")

    def __init__(self, text):
        self.original = text


class Foreign(str):
    def __new__(cls, value):
        return value

    def __init__(self, value):
        raise AssertionError("an unrelated __new__ result must skip __init__")


class Printable:
    def __str__(self):
        return Word("東京")


def current():
    for left, right in (("a", Word("b")), (Word("a"), "b"), (Word("a"), Word("b"))):
        result = left + right
        assert type(result) is str and result == "ab"
    pattern = Word("%s:%r")
    assert pattern.__mod__(("label", 1 << 70)) == "label:1180591620717411303424"
    assert str.__mod__(Word("%(key)s"), {"key": "value"}) == "value"
    assert str.__rmod__(Word("result"), "<%s>") == "<result>"
    assert Word("result").__rmod__(3) is NotImplemented

    class FormatValue:
        def __str__(self):
            return "value"

        def __rmod__(self, other):
            return "reflected"

    class ReflectedText(str):
        def __rmod__(self, other):
            return "reflected"

    assert "<%s>" % FormatValue() == "<value>"
    assert "<%s>" % ReflectedText("value") == "reflected"
    class Text(str):
        pass
    class Data(bytes):
        pass
    print("é" in Text("éclair"), "x" not in Text("éclair"))
    print(b"bc" in Data(b"abc"), 97 in Data(b"abc"))
    wrapped = Wrapped("Málaga")
    for i in range(1800):
        str(i)
    print(type(wrapped) is Wrapped, isinstance(wrapped, str), type(str(wrapped)) is str)
    print(wrapped, wrapped.original, wrapped.lower(), len(wrapped), wrapped == "<Málaga>")
    print(bool(Word()), bool(Word("x")), Foreign(42))
    encoded = Word(b"caf\xc3\xa9", "utf-8")
    print(encoded, type(encoded) is Word)
    plain = "plain"
    print(str.__new__(str, plain) is plain)
    subtype = Word(plain)
    print(str(subtype) is plain, str(subtype) is str(subtype))
    print(repr(subtype), [subtype])
    print(type(str(Printable())) is Word, str.__new__(Word, Printable()))
    for args in ((), (int, "text"), (12, "text")):
        try:
            str.__new__(*args)
        except TypeError as error:
            print(str(error))
