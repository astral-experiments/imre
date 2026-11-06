import json
from typing import Any


def key() -> str:
    return "n"


document = json.loads('{"rows": [{"n": 1}, {"n": 2}]}')
rows: list[dict[str, Any]] = document.get("rows", [])
print(rows[0][key()] < rows[1][key()])
rows[0] = rows[1]
print(rows[0] is document["rows"][1])
print(rows.pop()["n"], len(document["rows"]))

root = json.loads("[]")
root.append(root)
print(root[0] is root)
try:
    json.dumps(root)
except ValueError as error:
    print(str(error))
else:
    raise AssertionError("encoded a circular list")

for value in json.loads('[1, "text", null, {"key": 1}]'):
    print(isinstance(value, str))

text = '"a\x01b"'
print(repr(json.loads(text, strict=False)))
try:
    json.loads(text)
except json.JSONDecodeError:
    print("strict rejects")
else:
    raise AssertionError("accepted an unescaped control character")
