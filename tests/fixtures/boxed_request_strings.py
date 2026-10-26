class Index:
    def __index__(self):
        return -3


class DescriptorOwner:
    partition = str.partition


def current():
    text = "é🦊:東:🦊"
    print(str.find(text, ":"), str.partition(text, ":"), str.rpartition(text, ":"))
    print(str.partition is str.partition)
    try:
        DescriptorOwner().partition(":")
    except TypeError as error:
        print(error)
    for method in (str.find, str.partition, str.rpartition):
        try:
            method(None, ":")
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
    for value in ("\0", "A", "é", "東", "🦊", b"\xff", bytearray(b"Z"), "", "é🦊", b"", bytearray(b"ab"), None, 4):
        try:
            print(ord(value))
        except TypeError as error:
            print(error)
