def current():
    import gc

    events = []

    class Parts:
        def __getitem__(self, index):
            gc.collect()
            events.append(("get", index))
            return str(index)

    class Text(str):
        def partition(self, sep):
            events.append(("call", sep))
            return Parts()

    class Lookup:
        def __getattribute__(self, key):
            events.append(("lookup", key))
            return Text("x").partition

    def separator():
        events.append(("argument",))
        return "."

    def fields(value, sep):
        return (
            value.partition(sep)[0], value.partition(sep)[1], value.partition(sep)[2],
            value.rpartition(sep)[0], value.rpartition(sep)[1], value.rpartition(sep)[2],
        )

    assert fields("aa.é.bb", ".") == ("aa", ".", "é.bb", "aa.é", ".", "bb")
    assert fields("word", "::") == ("word", "", "", "", "", "word")
    assert fields("", "x") == ("", "", "", "", "", "")
    assert fields("::", "::") == ("", "::", "", "", "::", "")
    assert fields("ΣΣΣ", "ΣΣ") == ("", "ΣΣ", "Σ", "Σ", "ΣΣ", "")
    def materialized(value, sep):
        first = value.partition(sep)
        last = value.rpartition(sep)
        return first + last

    def outcome(operation, value, sep):
        try:
            return operation(value, sep)
        except (UnicodeError, TypeError) as error:
            return type(error), str(error)

    for value, separator_value in (("\ud800x.\udfff", "."), ("xy", "\ud800")):
        assert outcome(fields, value, separator_value) == outcome(materialized, value, separator_value)
    text = "".join(["left", "right"])
    sep = "".join(["ri", "gh"])
    assert text.partition(sep)[1] is sep
    assert text.partition("missing")[0] is text
    assert text.rpartition("missing")[2] is text
    assert Lookup().partition(separator())[2] == "2"
    assert events == [("lookup", "partition"), ("argument",), ("call", "."), ("get", 2)]
    assert Text("x").partition(".")[0] == "0"
    assert b"bytes".partition(b"t")[0] == b"by"
    for value, error_type in (("", ValueError), (1, TypeError)):
        try:
            text.rpartition(value)[1]
        except error_type:
            pass
        else:
            raise AssertionError("invalid separator")
