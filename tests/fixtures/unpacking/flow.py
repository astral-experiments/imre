# ruff: noqa: C408, PIE800
# Empty literal and constructor sources exercise distinct contextual paths.


def show(value: int | None) -> None:
    if value is None:
        print("none")
    else:
        print(value)


def unpack_guarded(values: list[int] | None) -> int:
    if values is None:
        return -1
    first, *rest = values
    return first + len(rest)


first: int | None = None
second: int | None = None
first, second = (1, 2)
print(first + second)
first, second = (None, 3)
print(first is None)
print(second + 1)
optional_number: int | None = None
required_number: int = 0
optional_number, required_number = [20, 21]
print(optional_number + required_number)
optional_number, required_number = [*[22, 23]]
print(optional_number + required_number)

optional: list[int | None] = [4, None, 6]
head, *tail = optional
show(head)
for value in tail:
    show(value)
print(unpack_guarded([7, 8]))
print(unpack_guarded(None))

texts: list[str] = []
made = texts = []
texts.append("context flows from a later target")
print(made[0])
words: list[str] = []
count = 0
((words, count),) = [([], 1)]
print(len(words))
print(count)
((words, count),) = [(["nested list context"], 2)]
print(words[0])
print(count)

numbers = [*(saved := [9, 10]), len(saved)]
print(saved[1])
print(numbers[2])
empty_strings: list[str] = [*(remembered := [])]
remembered.append("bound even when empty")
print(len(empty_strings))
print(remembered[0])
letters: list[str] = [*[], "eleven"]
print(letters[0])
states: list[int | None] = [*[12, 13], None]
for value in states:
    show(value)
tuple_states: list[int | None] = [*(14, None, 15)]
for value in tuple_states:
    show(value)
original_numbers = [16]
promoted: list[int | None] = [*original_numbers, None]
original_numbers.append(17)
print(len(promoted))
print(len(original_numbers))
show(promoted[0])
nested: list[list[str]] = [*([],)]
nested[0].append("nested tuple context")
print(nested[0][0])

source = {"old": 16}
mapping: dict[str, int | None] = {**source, "missing": None}
source["old"] = 17
show(mapping["old"])
show(mapping["missing"])
print(source["old"])

bound = {**(captured := {"value": 18}), "size": len(captured)}
print(captured["value"])
print(bound["size"])
empty: dict[str, int] = {**{}, **dict()}
print(len(empty))
empty_set: set[str] = {*[]}
print(len(empty_set))
empty_enumeration: list[tuple[int, str]] = [*enumerate([])]
print(len(empty_enumeration))
empty_items: list[tuple[str, int]] = [*{}.items()]
print(len(empty_items))
empty_keys: list[str | None] = [*{}, *dict()]
print(len(empty_keys))
