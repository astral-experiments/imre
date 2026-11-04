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
    saved = "kept".upper
    assert saved.__self__ == "kept" and saved() == "KEPT"
    try:
        "ab".upper(1)
    except TypeError:
        pass
    else:
        raise AssertionError("extra argument accepted")
    return 0
