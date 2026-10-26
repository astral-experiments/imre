import json
from typing import Any


def key_length(value: str) -> int:
    return len(value)


document = r'[{"b_é":1,"a_key":2,"b_\u00e9":3},{"\u0061_key":4,"b_é":5}]'
rows: list[dict[str, Any]] = json.loads(document)
insertion = list(rows[0])
left = sorted(rows[0])
right = sorted(rows[1])
print(insertion)
print(left)
print(right)
print(left[0] is right[0])
print(left[1] is right[1])
print(left[1] is insertion[0])
print(sorted(rows[1], reverse=True))
print(sorted(rows[1], key=key_length))
empty: dict[str, Any] = json.loads('{}')
print(sorted(empty))
again: list[dict[str, Any]] = json.loads(document)
new_keys = sorted(again[0])
print(new_keys == left)
print(new_keys[0] is left[0])
print(new_keys[1] is left[1])
