class Collection:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values

    def items(self, extra: int) -> list[int]:
        return [*self.values, extra]


def show(values: list[int]) -> None:
    for value in values:
        print(value)


show([0, *[1, 2], 3, *(4, 5), *range(6, 9)])
for character in [*"é🙂", "last"]:
    print(character)
show([*b"\x00\x80\xff"])
for key in [*{"second": 2, "first": 1}]:
    print(key)

set_values = [*{10, 20, 30}]
total = 0
for value in set_values:
    total += value
print(len(set_values))
print(total)

pairs = [*enumerate(["left", "right"], 4)]
for number, text in pairs:
    print(number)
    print(text)
entries = [*{"first": 1, "second": 2}.items()]
for key, value in entries:
    print(key)
    print(value)

numbers = {*[1, 1, 2], *(2, 3), *range(3, 5), *b"\x00"}
print(len(numbers))
for value in range(5):
    print(value in numbers)
characters = {*"abaca"}
print(len(characters))
print("a" in characters)
print("c" in characters)
keys = {*{"first": 1, "second": 2}}
print("first" in keys)
print("second" in keys)
bytes_set = {*[b"a", b"b"], b"a"}
print(len(bytes_set))
print(b"b" in bytes_set)

left = {"first": 1, "shared": 2}
right = {"shared": 3, "last": 4}
combined = {**left, "middle": 5, **right, "first": 6}
for key, value in combined.items():
    print(key)
    print(value)
print(left["first"])
print(right["shared"])

nested = {"row": [7]}
copy = {**nested}
copy["row"].append(8)
print(nested["row"][1])
copy["own"] = [9]
print(len(nested))
print(len(copy))
collection = Collection([10, 11])
show([*collection.items(12)])
for floating in [*[-0.0, float("nan")], float("inf")]:
    print(floating)
show([*[1000000000000000000000000000000], -2000000000000000000000000000000])
