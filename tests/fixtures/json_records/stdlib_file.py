import json
from dataclasses import dataclass
from typing import Any


@dataclass
class Page:
    items: list[dict[str, Any]]


def read_rows() -> list[dict[str, Any]]:
    with open("rows.json", "r", encoding="utf-8") as source:
        document = json.load(source)
    return document.get("rows", [])


def total(row: dict[str, Any]) -> int:
    result = row.get("left", 0) + row.get("right", 0)
    assert isinstance(result, int)
    return result


def rows(text: str) -> list[dict[str, Any]]:
    with open("rows.json", "w", encoding="utf-8") as out:
        out.write(text)
    return read_rows()


def default() -> int:
    print("evaluating the default")
    raise ValueError("from default")


for item in rows('{"rows":[{},{"left":true},{"left":42,"left":100,"right":-7},'
                 '{"left":92233720368547758081234567,"right":2}]}'):
    print(total(item))
print(len(rows('{"unrelated": true}')))
for item in rows('{"rows":[null,{"left":1}]}'):
    try:
        item.get("left", default())
    except AttributeError as error:
        print(str(error))
    except ValueError as error:
        print(str(error))

for item in rows('{"rows":[{"left":"abc","right":"def"},{"left":[],"right":[]},'
                 '{"left":1.25,"right":3}]}'):
    try:
        total(item)
    except AssertionError:
        print("not an integer")

for item in rows('{"rows":[{"left":"abc"},{"left":null},{"left":[]},{"right":{}}]}'):
    try:
        total(item)
    except TypeError as error:
        print(str(error))


first = Page(rows('{"rows":[{"a":0,"b":null,"a":[true,1.5,"hi"]}]}'))
second = Page(rows('{"rows":[{"b":null,"a":[1,1.5,"hi"]}]}'))
print(first == second)
print(repr(first))
print(Page(rows('{"rows":[{"n":9007199254740993}]}')) ==
      Page(rows('{"rows":[{"n":9007199254740992.0}]}')))
