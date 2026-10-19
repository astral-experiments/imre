# ruff: noqa: PLW0120
# Else suites exercise every supported cursor's exhaustion edge.


def ranges(start: int, stop: int, step: int, break_at: int) -> None:
    count = 0
    for value in range(start, stop, step):
        count += 1
        if value == break_at:
            break
        if value == 1:
            continue
        print(value)
    else:
        print("range exhausted")
    print(count)


ranges(0, 0, 1, -1)
ranges(0, 4, 1, -1)
ranges(0, 4, 1, 2)
ranges(3, 0, -1, -1)
ranges(1000000000000000000000000000000, 1000000000000000000000000000002, 1, -1)
empty: list[int] = []
for value in empty:
    print("unreachable")
else:
    print("empty list")
values = [1, 2]
for value in values:
    print(value)
    if value == 1:
        values.append(3)
else:
    print(len(values))
for character in "é🌍":
    print(character)
else:
    print("string exhausted")
for character in "":
    print("unreachable")
else:
    print("empty string")
for byte in b"\0\xff":
    print(byte)
else:
    print("bytes exhausted")
for index, word in enumerate(["a", "b"], 7):
    print(index)
    print(word)
else:
    print("enumerate exhausted")
mapping = {"a": 1, "b": 2}
for key, entry in mapping.items():
    print(key)
    print(entry)
else:
    print("items exhausted")
for key in mapping:
    print(key)
else:
    print("keys exhausted")
for entry in mapping.values():
    print(entry)
else:
    print("values exhausted")
total = 0
for member in {3, 1, 2}:  # noqa: PLC0208 - exercise a temporary set cursor.
    total += member
else:
    print(total)

owned = [[1000000000000000000000000000000]]
for row in owned:
    owned.pop()
    print(row[0])
else:
    print(len(owned))
