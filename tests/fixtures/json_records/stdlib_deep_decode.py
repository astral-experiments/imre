import json


# These inputs previously hit the typed schema parser's 128-container boundary.
for depth in [129, 257]:
    text = ('{"key":[' * depth) + '315' + (']}' * depth)
    root = json.loads(text)
    item = root["key"][0]
    for offset in range(1, depth):
        item = item["key"][0]
    leaf: int = item
    print(leaf)
    print(root == json.loads(text))
    print(root != json.loads(text.replace("315", "316")))
    print(repr(root) == repr(json.loads(text)))
    rows = json.loads("[0," + text + ",2]")
    rows.append(rows[1])
    print(rows == json.loads("[0," + text + ",2," + text + "]"))

try:
    json.loads("[" * 500_000 + "0" + "]" * 500_000)
except RecursionError:
    print("RecursionError")

try:
    json.loads("[" * 160 + "1e" + "]" * 160)
except json.JSONDecodeError as exc:
    print(exc.pos)
    print(exc.lineno)
    print(exc.colno)
