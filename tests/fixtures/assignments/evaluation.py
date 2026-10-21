class Item:
    text: str
    values: list[int]

    def __init__(self, text: str, values: list[int]) -> None:
        self.text = text
        self.values = values


def join(first: str, second: str, third: str) -> str:
    return first + ":" + second + ":" + third


def rebound_parameter(text: str) -> str:
    print(join(text, (text := "middle"), text))
    print(join(third=text, first=(text := "first"), second=text))
    return text


def accept_optional(*, number: int | None) -> None:
    if number is None:
        print(None)
    else:
        print(number)


print(rebound_parameter("original"))
text = "owned " + str(17)
print(text + (text := "replacement") + text)
print(f"{text}:{(text := 'formatted')}:{text}")
number = int("100000000000000000000000000000000000000000000000000000000")
print(number + (number := number + 1) + number)
item = Item("old", [10])
alias = item
item.text = (item := Item("new", [20])).text + ":set"
print(item.text)
print(alias.text)
values = [10, 20]
previous = values
values[(index := 1)] = (values := [30, 40])[0]
print(values[0])
print(values[1])
print(previous[1])
print(index)
(target := values)[(position := 0)] = (replacement := 99)
print(target[position])
print(replacement)
words = {"key": "old"}
old_key = "key"
print(words[(old_key := "key")])
mapping = {(key := "owned " + str(1)): (payload := [2])}
print(mapping[key][0])
print(payload[0])
source = [0, 1, 2, 3, 4]
print(len(source[(start := 1) : (stop := 4) : (step := 2)]))
print(start + stop + step)
optional: int | None = None
print((optional := 4) + 1)
print(optional + 1)
print((optional := 3 if len(text) > 0 else 4) + 1)
print((optional := None))
accept_optional(number=(new_number := 7))
print(new_number + 1)
nullable_values: list[int | None] = [1]
nullable_values[(slot := 0)] = (missing := None)
print(nullable_values[slot] is None)
print(missing)
pair = (1, "before")
print(pair[1])
print((pair := (2, "after"))[1])
print(pair[0])
