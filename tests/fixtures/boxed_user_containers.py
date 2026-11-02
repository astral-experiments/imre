import collections
from collections import UserDict, UserList, UserString
from collections.abc import Mapping


class Default(UserDict):
    def __missing__(self, key):
        return ("missing", key)


class MappingItems(UserDict):
    def items(self):
        return [("custom", self["key"])]


class ListChild(UserList):
    pass


class TextChild(UserString):
    pass


def current():
    assert UserDict is collections.UserDict and UserList is collections.UserList
    assert UserString is collections.UserString
    for cls in (UserDict, UserList, UserString):
        assert cls.__module__ == "collections"
    d = Default(a=1)
    assert list(d.items()) == [("a", 1)] and UserDict(d).data is not d.data
    assert d["x"] == ("missing", "x") and d.get("x") is None
    assert MappingItems(key=1) == {"custom": 1}
    assert UserDict({"custom": 1}) == MappingItems(key=1)
    assert Mapping.__eq__(d, object()) is NotImplemented and UserDict.__hash__ is None
    items = ListChild("ab")
    assert items[1:] == ["b"] and type(items[1:]) is ListChild
    text = UserString("ééé")
    assert UserString(text).data == "ééé" and text.count(UserString("éé")) == 1
    assert UserString("a\udcff" * 3).count(UserString("\udcffa")) == 2
    assert type(TextChild("word")[1:]) is TextChild

    for cls, value, method in ((str, "", "rindex"), (dict, {}, "__ror__"), (list, [], "__imul__")):
        names = dir(cls)
        assert names == dir(value) and method in names
        names.remove(method)
        assert method in dir(cls)
