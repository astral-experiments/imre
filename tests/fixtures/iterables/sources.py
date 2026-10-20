original = [1, 2]
copied = list(original)
copied.append(3)
original[0] = 9
print(copied[0])
print(len(original))
print(len(copied))

child = [4]
children = [child, child]
copied_children = list(children)
children = []
child.append(5)
for values in copied_children:
    print(values[1])

for value in list(range(5, -2, -2)):
    print(value)
for value in list((7, 8)):
    print(value)
for value in list(b"\x00\x80\xff"):
    print(value)
for character in list("Aé🐍e\u0301\0"):
    print(len(character))
    print(character)

mapping = {"first": 11, "second": 12}
for name in list(mapping):
    print(name)
for key, value in list(mapping.items()):
    print(key)
    print(value)
for index, name in list(enumerate(["one", "two"], 7)):
    print(index)
    print(name)

unordered = list({2, 4, 6})
print(len(unordered))
print(sum(unordered))
optional: list[int | None] = list((1, None, 3))
for item in optional:
    if item is None:
        print("none")
    else:
        print(item)
