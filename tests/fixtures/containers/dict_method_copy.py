def select(mapping: dict[str, list[int]]) -> dict[str, list[int]]:
    print("receiver")
    return mapping


mapping = {"first": [1], "second": [2]}
duplicate = select(mapping).copy()
print(duplicate is mapping)
print(duplicate["first"] is mapping["first"])
duplicate["first"].append(3)
del duplicate["second"]
duplicate["third"] = [4]
print(mapping["first"])
for key in mapping:
    print(key)
for key in duplicate:
    print(key)

empty: dict[int, str] = {}
copied = empty.copy()
copied[4] = "four"
print(len(empty))
print(copied[4])
