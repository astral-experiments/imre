# ruff: noqa: C400
# Explicit generator consumers exercise nested pull-state composition.


def show(values: list[int]) -> None:
    for value in values:
        print(value)


show(list(map(int, (text for text in reversed(["1", "2", "3"])))))
show(list(value + 1 for value in map(int, reversed("٣２1"))))
print(sum(value for index, value in enumerate(map(int, reversed("1234"))) if index % 2))
for index, pair in enumerate(
    zip(reversed([1, 2, 3]), (value * 2 for value in range(3))), 5
):
    print(index)
    print(pair[0])
    print(pair[1])
for left, right in zip(
    (value + 1 for value in (inner * 2 for inner in range(4))),
    reversed(range(4)),
    strict=True,
):
    print(left + right)

show([left + right for left, right in zip([1, 2], reversed([3, 4]), strict=True)])
mapping = {str(left): right for left, right in zip([1, 2], (value for value in [4, 5]))}
print(mapping["1"])
print(mapping["2"])
unique = {value for value in (index % 2 for index in range(5))}
print(len(unique))
print(0 in unique)
print(1 in unique)

# A nested generator's leftmost source is constructed only when its outer body runs.
show(
    list(
        inner for outer in [1, 3] for inner in (value + outer for value in range(outer))
    )
)
show(list(sum(value for value in range(outer)) for outer in reversed([1, 2, 3])))
print(any(all(value > 0 for value in row) for row in [[0], [1, 2], [3]]))
print(all(any(value > 0 for value in row) for row in [[0, 1], [2], []]))

# zip pulls every source left-to-right for each row, even if sources share a list.
values = [1, 2, 3]
for forward, backward in zip(values, reversed(values), strict=True):
    print(forward)
    print(backward)
    values[1] = 20
