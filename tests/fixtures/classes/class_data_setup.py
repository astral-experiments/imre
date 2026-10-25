import unittest


def number(value: int) -> int:
    return value


def words(value: str) -> list[str]:
    return [value]


class Reader(unittest.TestCase):
    def read(self) -> int:
        return self.value


class WithDefault(Reader):
    value = 4

    def setUp(self):
        self.value = 6


class WithoutDefault(Reader):
    def setUp(self):
        self.value = 8


class Nullable(unittest.TestCase):
    selected = None

    def setUp(self):
        self.selected = number(7)


class WordReader(unittest.TestCase):
    def read(self) -> list[str]:
        return self.value


class Words(WordReader):
    value = words("class")

    def setUp(self):
        self.value = words("instance")


class DirectWords(WordReader):
    def setUp(self):
        self.value = words("sibling")


def clear_words(case: WordReader):
    case.value = []


def remove(reader: Reader):
    del reader.value


def show(value: int | None):
    if value is None:
        print("none")
    else:
        print(value)


first = WithDefault()
fresh = WithDefault()
other = WithoutDefault()
print(first.read())
try:
    other.read()
except AttributeError as exc:
    print(str(exc))
first.setUp()
other.setUp()
print(first.read())
print(other.read())
WithDefault.value = 11
print(fresh.read())
print(first.read())
remove(first)
remove(other)
print(first.read())
try:
    other.read()
except AttributeError as exc:
    print(str(exc))
case = Nullable()
show(case.selected)
case.setUp()
show(case.selected)
case.selected = None
show(case.selected)
Nullable.selected = number(12)
del case.selected
show(case.selected)
w = Words()
kept = w.value
w.setUp()
print(w.value)
print(kept is Words.value)
spare = DirectWords()
spare.setUp()
print(spare.read())
clear_words(w)
clear_words(spare)
print(w.read())
print(spare.read())
print(w.value is spare.value)
del w.value
print(w.value is kept)
