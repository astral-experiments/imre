class Box:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def source(box: Box) -> list[int]:
    print("source once")
    box.values.append(21)
    box.values = [90]
    return [22]


def extended_pair(value: int) -> bytes:
    result: list[int] = []
    result.extend(divmod(value, 256))
    return bytes(result)


values = [1, 2]
alias = values
print(values.extend(values))
alias.extend(alias)
for value in values:
    print(value)
values.extend(())
values.extend([])
values.extend((3, 4))
values.extend(range(8, 5, -1))
values.extend(b"\x00\x80\xff")
for value in values:
    print(value)

texts: list[str] = []
texts.extend("é🐍e\u0301")
texts.extend(("first", "second"))
texts.extend({"third": 3, "fourth": 4})
for text in texts:
    print(text)

optional: list[int | None] = []
optional.extend([1, 2])
optional.extend((None, 3))
optional.extend(optional)
for optional_value in optional:
    if optional_value is None:
        print("none")
    else:
        print(optional_value)

pairs: list[tuple[int, str]] = []
pairs.extend(enumerate(["a", "b"], 9))
pairs.extend({11: "c", 12: "d"}.items())
for key, paired_text in pairs:
    print(key)
    print(paired_text)

from_set: list[int] = []
from_set.extend({3, 1, 2})
total = 0
for value in from_set:
    total += value
print(total)
print(len(from_set))

child = [10]
children: list[list[int]] = []
children.extend((child, child))
children.extend(children)
child.append(11)
for item in children:
    print(item[1])


box = Box([20])
before = box.values
box.values.extend(source(box))
for value in before:
    print(value)
print(box.values[0])

try:
    before.extend(range(0, 3, 0))
except ValueError:
    print(len(before))

huge = 1 << 140
owned = [huge]
owned.extend(owned)
owned.extend((huge,))
for value in owned:
    print(value)

bytes_out: list[int] = []
bytes_out.extend(divmod(65535, 256))
print(bytes(bytes_out))
print(extended_pair(65535))
