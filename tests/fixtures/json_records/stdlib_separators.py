import json

sep = (" ; ", " => ")
print(json.dumps({"z": {"big": 1, "a": 5}, "a": {}}, sort_keys=True, separators=sep))
print(json.dumps({10: "ten", -2: "minus", 2: "two"}, sort_keys=True, separators=["|", ":"]))
print(json.dumps(json.loads('{"z":[1,2],"a":2,"z":3}'), sort_keys=True, separators="⋅:"))
print(json.dumps([1, 2], separators=("", "")))
print(json.dumps([1, 2], indent=1, separators=(" ;", " : ")))
print(json.dumps({"x": 1, "y": 2}, separators={"|": 0, "=": 0}))
sep_list = [1, 2]
try:
    json.dumps({"x": 1}, separators=sep_list)
except TypeError as exc:
    print(str(exc))
try:
    json.dumps([1, 2], separators=[";", False])
except TypeError as exc:
    print(str(exc))
try:
    json.dumps([1, 2], separators=b",:")
except TypeError as exc:
    print(str(exc))
try:
    json.dumps(None, separators=[])
except ValueError as exc:
    print(str(exc))
try:
    json.dumps(None, separators={"!"})
except ValueError as exc:
    print(str(exc))
for bad in [(b",", ":"), (b";", "=")]:
    try:
        json.dumps({1: 2, 3: 4}, separators=bad)
    except TypeError as exc:
        print(str(exc))
with open("sorted.json", "w") as sink:
    json.dump(json.loads('{"z":true,"a":{"b":1,"a":2}}'), sink, indent=2,
              sort_keys=True, separators=(",", ":"))
with open("sorted.json") as source:
    print(source.read())
