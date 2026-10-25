import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Self


@dataclass(kw_only=True, slots=True)
class Entry:
    check: str
    path: Path
    line: int

    @classmethod
    def from_row(cls, row: dict[str, Any]) -> Self:
        return cls(
            check=row["name"], path=Path(row["location"]["path"]),
            line=row["location"]["position"]["line"],
        )


def select(text: str) -> list[Entry]:
    return [Entry.from_row(row) for row in json.loads(text) if row["severity"] == "major"]


def subview(text: str) -> dict[str, Any]:
    return json.loads(text)[0]["child"]


def check_contract(text: str) -> None:
    try:
        for entry in select(text):
            # Give the CPython oracle the same exact field contracts.
            if not isinstance(entry.check, str) or isinstance(entry.line, bool) or not isinstance(entry.line, int):
                raise TypeError("invalid field")
    except TypeError:
        print("invalid field")


big = ""
for index in range(40):
    big += "1234567890"
data = (
    '[{"severity":null},{"severity":false},{"severity":[]},{"severity":{"x":1}},'
    '{"severity":"minor","location":"ignored"},'
    '{"severity":"major","name":"first","name":"α","location":{"path":"first.py","position":{"line":'
    + big
    + '}}},{"severity":"major","name":"second","location":{"path":"second.py","position":{"line":-7}}}]'
)
for entry in select(data):
    print(f"{entry.check}:{entry.path.name}:{entry.line}")

for document in ['[{},', '[{}]', '[1]', '[{"severity":"major"}]']:
    try:
        select(document)
    except json.JSONDecodeError:
        print("syntax")
    except KeyError as error:
        print(str(error))
    except TypeError:
        print("subscript")

for fields in ['"name":false,"location":{"path":"ok","position":{"line":1}}',
               '"name":"ok","location":{"path":"ok","position":{"line":true}}',
               '"name":"ok","location":{"path":"ok","position":{"line":1.0}}']:
    check_contract('[{"severity":"major",' + fields + '}]')

child = subview(r'[{"child":{"":"empty key","\u0000":"NUL key","value":null}}]')
first: str = child[""]
second: str = child["\x00"]
empty: int | None = child["value"]
print(f"{first} {second} {empty is None}")
for row in json.loads('[null,false,0,0.0,-1,[],[0],{},{"a":0},"","yes"]'):
    print(f"{bool(row)} {row is None} {'yes' != row}")

# Error positions count Unicode characters. Malformed numbers end at their
# valid prefix; unterminated strings point back to the opening quote.
for document in ["", '["🌗",\n falsex]', '["é" 2]', '{"key"\n1}',
                 r'["bad\q"]', r'["bad\u12xz"]', '["open', '[1e]', '[] extra']:
    try:
        select(document)
    except json.JSONDecodeError as error:
        print(str(error))
        print(f"{error.msg}|{error.pos}|{error.lineno}|{error.colno}")
        print(error.doc == document)
