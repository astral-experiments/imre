import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Self

# The definition and two reader bodies come from the pinned Ruff memory report.
@dataclass(slots=True, kw_only=True)
class MemoryReport:
    """Memory report from a single ty run."""

    total_bytes: int
    struct_metadata_bytes: int
    struct_fields_bytes: int
    memo_metadata_bytes: int
    memo_fields_bytes: int
    structs: list[dict[str, Any]] = field(default_factory=list)
    queries: list[dict[str, Any]] = field(default_factory=list)

    @classmethod
    def from_json(cls, path: Path) -> Self:
        """Load a memory report from a JSON file."""
        with open(path) as f:
            data = json.load(f)
        return cls(
            total_bytes=data["total_bytes"],
            struct_metadata_bytes=data["struct_metadata_bytes"],
            struct_fields_bytes=data["struct_fields_bytes"],
            memo_metadata_bytes=data["memo_metadata_bytes"],
            memo_fields_bytes=data["memo_fields_bytes"],
            structs=data.get("structs", []),
            queries=data.get("queries", []),
        )

def item_total_bytes(item: dict[str, Any]) -> int:
    """Get total bytes (metadata + fields) for a struct or query item."""
    result = item.get("metadata_bytes", 0) + item.get("fields_bytes", 0)
    assert isinstance(result, int)
    return result

def bad_default() -> int:
    print("evaluated default")
    raise ValueError("bad default")


text = '{"total_bytes": 1,"total_bytes": 999999999999999999999999999999, "struct_metadata_bytes": 0, "struct_fields_bytes": 0, "memo_metadata_bytes": 2, "memo_fields_bytes": 3, "structs": [{"metadata_bytes": 91, "fields_bytes": 7}, {"fields_bytes": true}]}'
with open("report.json", "w") as out:
    out.write(text)
report = MemoryReport.from_json(Path("report.json"))
print(report.total_bytes)
for item in report.structs:
    print(item_total_bytes(item))
print(len(report.queries))
print(report == MemoryReport.from_json(Path("report.json")))
print(repr(report))
document = json.loads(text)
# Exercise both sides of the bounded root-field index with an escaped duplicate key.
for extra_field in ["", ',"h":8']:
    fields = r'{"\u0061":1,"a":999999999999999999999999999999,"b":2,"c":3,"d":4,"e":5,"f":6,"g":7'
    print(json.dumps(json.loads(fields + extra_field + "}")))
try:
    json.loads(r'{"\ud800":0} extra')
except json.JSONDecodeError as error:
    print(str(error))
for raw_key in document:
    key: str = raw_key
    print(key)
for child in document["structs"]:
    value: int = child.get("metadata_bytes", 8)
    print(value)
nested = json.loads('[{"items":[["🌗", "β"]]}]')[0]["items"]
label: str = nested[0][-1]
print(label)
print(len(nested))
print(nested[0][0] == "🌗")
for received in ["null", "123", '"élève"']:
    loaded = json.loads(received)
    try:
        print(len(loaded))
    except TypeError as error:
        print(str(error))
root = json.loads('[{"label":"one"},{"label":"two"}]')
alias = root
before = root[0]
root.append(before)
print(len(alias))
print(alias.pop()["label"] == "one")
print(root[-1]["label"] == "two")
try:
    json.loads('{"a":1}')[999999999999999999999999]
except KeyError as error:
    print(str(error))

print(json.dumps(document["structs"], indent=1))
print(json.dumps(root))
root[1] = before
print(json.dumps(root))
for encoded in ['[Infinity,-Infinity,NaN,0e0,0.000e00,0.0,"NaN"]', '[{},{"n":NaN}]', "NaN"]:
    print(json.dumps(json.loads(encoded)))
for invalid in ["[NaN,]", "Infinity extra", "[nan]", '["NaN", Infin]',
                "-NaN", "1NaN", "NaN1", "[0Infinity]", "[Infinity2]"]:
    try:
        json.loads(invalid)
    except ValueError as error:
        print(str(error))


for row in json.loads("[null,{}]"):
    try:
        row.get("absent", bad_default())
    except AttributeError as error:
        print(str(error))
    except ValueError as error:
        print(str(error))
with open("report.json") as closed:
    pass
try:
    json.load(closed)
except ValueError as error:
    print(str(error))


for with_index in ["[]", '"é"']:
    try:
        json.loads(with_index)[999999999999999999999999999]
    except IndexError as error:
        print(str(error))
