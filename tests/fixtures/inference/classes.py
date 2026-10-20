class Row:
    value: int
    tags: list[str]

    def __init__(self, value, tags):
        self.value = value
        self.tags = tags

    def add(self, amount=2):
        self.value += amount
        return self.value

    def tag(self, text):
        self.tags.append(text)

    def forwarded(self, value):
        return self.add(value)

    def unused_current(self):
        return self.value


class ItemsBox:
    marker: int

    def __init__(self, marker: int) -> None:
        self.marker = marker

    def items(self, value):
        return [value]


def create(value):
    return Row(value, ["created"])


row = create(4)
print(row.add())
print(row.forwarded(3))
print(row.tag("added"))
for text in row.tags:
    print(text)
empty = Row(10, [])
print(len(empty.tags))
print(empty.add(amount=7))
box = ItemsBox(1)
expanded = [*box.items(2)]
print(expanded[0])
extended: list[int] = []
extended.extend(box.items(3))
print(extended[0])
