"""Stable input lengths survive nested scopes; mutation keeps live iteration."""


def nested(values: list[int], words: list[str], repeats: int) -> int:
    alias = values
    text_alias = words
    total = len(alias) + len(text_alias)
    while repeats > 0:
        total += sum(
            sum(
                left + right for left, right in zip(alias, reversed(alias), strict=True)
            )
            for index in range(2)
        )
        total += sum(len(word) for word in (item for item in text_alias))
        repeats -= 1
    return total + len(alias) + len(text_alias)


def reset_alias(values: list[str], clear: bool) -> int:
    alias = values
    if clear:
        alias = []
    total = sum(len(word) for word in alias)
    alias = values
    return total + sum(len(word) for word in alias)


def reset_parameter(values: list[int], clear: bool) -> int:
    if clear:
        values = []
    return sum(value for value in values) + len(values)


def append_number(values: list[int], value: int) -> int:
    if value == 1:
        values.append(3)
    return 0


def delegated_append(values: list[int], value: int) -> int:
    return append_number(values, value)


def growing_numbers(values: list[int]) -> int:
    return sum(value + delegated_append(values, value) for value in values)


def append_word(values: list[str], word: str) -> int:
    if word == "a":
        values.append("ccc")
    return 0


def growing_words(values: list[str]) -> int:
    return sum(len(word) + append_word(values, word) for word in values)


def remove_word(values: list[str]) -> int:
    values.pop()
    return 0


def delegated_remove(values: list[str]) -> int:
    return remove_word(values)


def shrinking_words(values: list[str]) -> int:
    return sum(len(word) + delegated_remove(values) for word in values)


def acquired_after_mutation(values: list[int]) -> list[int]:
    values.append(3)
    return values


def construction_order(values: list[int]) -> int:
    return sum(
        left + right
        for left, right in zip(
            values, (item for item in acquired_after_mutation(values)), strict=True
        )
    )


for repeats in range(4):
    print(nested([], [], repeats))
    print(nested([1, 2, (1 << 100)], ["a", "two"], repeats))
for clear in [False, True]:
    print(reset_alias(["a", "two"], clear))
    print(reset_parameter([1, (1 << 100)], clear))
print(growing_numbers([1, 2]))
print(growing_words(["a", "bb"]))
print(shrinking_words(["a", "bb", "ccc", "dddd"]))
print(construction_order([1, 2]))
