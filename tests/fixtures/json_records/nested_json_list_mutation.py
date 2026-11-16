import json
from typing import Any

document = json.loads('{"rows": [{"id": 1}, {"id": 2}]}')
rows: list[dict[str, Any]] = document.get("rows", [])
last = rows[-1]
removed = rows.pop()
assert removed is last
assert removed == {"id": 2}
assert document["rows"] is rows
assert document["rows"] == [{"id": 1}]

rows = json.loads("{}").get("rows", [])
try:
    rows.pop()
except IndexError:
    pass
else:
    raise AssertionError("pop on the empty default succeeded")
