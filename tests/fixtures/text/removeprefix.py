def trim(value: str, prefix: str):
    return value.removeprefix(prefix)


print(trim("&'a Node", "&'a "))
print(trim("Node", "&'a "))
print(trim("ππ", "π"))
print(trim("unchanged", ""))


class Word(str):
    def removeprefix(self, prefix: str):
        return ("original", prefix)


def replacement(self, prefix: str):
    return ("replacement", prefix)


def rebind() -> str:
    Word.removeprefix = replacement
    return "pre"


word = Word("prefix")
print(trim(word, "pre"))
print(word.removeprefix(rebind()))
print(trim(word, "pre"))
print(str.removeprefix(word, "pre"))


marker = object()


class Marker(str):
    def removeprefix(self, prefix: str):
        return marker


class Number(str):
    def removeprefix(self, prefix: str):
        return 20


print(trim(Marker("prefix"), "pre") is marker)
print(trim(Number("prefix"), "pre") * 2 + 2)
