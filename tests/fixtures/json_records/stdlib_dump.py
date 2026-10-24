import json


class Thing:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


print(json.dumps({}))
print(json.dumps([]))
print(json.dumps((None, True, 123456789012345678901234567890, -0.0, "🌗\n")))
print(json.dumps({10: ["á", "🙂"], -123456789012345678901234: ["β"]}, indent="\t", ensure_ascii=False))
print(json.dumps([1e999, -1e999, 1e999 - 1e999]))
for value in [1e999, -1e999, 1e999 - 1e999]:
    try:
        print(json.dumps(value, allow_nan=False))
    except ValueError as error:
        print(str(error))
try:
    print(json.dumps(Thing(3)))
except TypeError as error:
    print(str(error))
try:
    print(json.dumps(1, indent=9999999999999999999999999))
except OverflowError as error:
    print(str(error))

with open("dump.json", "w", encoding="utf-8") as out:
    print(json.dump({2: 3, 4: 5}, out, indent=0))
    try:
        json.dump({"rows": [1.0, 1e999]}, out, allow_nan=False)
    except ValueError as error:
        print(str(error))
    try:
        json.dump((1, b"x"), out)
    except TypeError as error:
        print(str(error))
with open("dump.json", encoding="utf-8") as saved:
    print(saved.read())
