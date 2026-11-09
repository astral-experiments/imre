import gc


def current():
    events = []

    class Descriptor:
        def __get__(self, instance, owner):
            events.append("lookup")
            return lambda value: value + 1

    class Receiver:
        upper = Descriptor()

    def replace():
        events.append("argument")
        Receiver.upper = lambda self, value: value + 10
        return 3

    assert Receiver().upper(replace()) == 4
    assert events == ["lookup", "argument"]
    assert Receiver().upper(3) == 13

    class Text(str):
        def split(self, separator):
            return ("overridden", separator)

        def __contains__(self, item):
            return False

    assert Text("abc").split("-") == ("overridden", "-")
    owners = ["-".join(["a", "b"])]

    def drop_owner():
        owners.clear()
        gc.collect()
        return "-"

    assert owners[0].split(drop_owner(), 1) == ["a", "b"]
    assert "x-y-x".replace(" ax ".strip().removeprefix("a"), "ZY".lower().replace("y", "")) == "z-y-z"
    assert ("ab" + chr(0xd800)).endswith(chr(0xd800))
    assert "ab".maketrans("a", "z") == {97: 122}
    assert "a,,b".split(sep=",") == ["a", "", "b"]
    assert "ab".replace(*("a", "z")) == "zb"
    fold = str.casefold
    word = "ΣςStraße\ud800"
    bound = word.casefold
    assert bound.__self__ is word
    assert bound() == fold(word) == "σσstrasse\ud800"
    assert sorted(["Straße", "STRASSE", "A"], key=fold) == ["A", "Straße", "STRASSE"]
    saved = "kept".upper
    assert saved.__self__ == "kept" and saved() == "KEPT"
    assert str.upper.__get__("kept", str)() == "KEPT"

    class UpperName(str):
        def __hash__(self):
            return hash("upper")

        def __eq__(self, other):
            return other == "upper"

    assert getattr("kept", UpperName("different"))() == "KEPT"
    assert word.__getattribute__("upper").__self__ is word
    assert str.__add__(word, "") is word

    contains = str.__contains__
    assert type(contains).__name__ == "wrapper_descriptor"
    assert contains("ΣStraße", "Straße")
    assert not word.__contains__("STRASSE")
    bound = contains.__get__(word, str)
    assert bound.__self__ is word and bound("e\ud800")
    text = Text("Straße")
    assert "Straße" not in text and contains(text, "Straße")
    try:
        contains(word, 1)
    except TypeError as error:
        assert str(error) == "'in <string>' requires string as left operand, not int"
    else:
        raise AssertionError("non-string operand accepted")

    class Reflected:
        def __radd__(self, other):
            return other.upper()

    assert "abc" + Reflected() == "ABC"
    try:
        str.__add__("abc", Reflected())
    except TypeError:
        pass
    else:
        raise AssertionError("direct concatenation called the reflected operand")
    try:
        "ab".upper(1)
    except TypeError:
        pass
    else:
        raise AssertionError("extra argument accepted")
    return 0
