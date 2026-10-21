def show(values: list[int]) -> None:
    print(len(values))
    print(sum(values))
    for value in values:
        print(value)


def change(values: list[int]) -> None:
    values[1] = 1 << 100
    values.append(-7)


huge = 1 << 100
for source in [
    [0, 1, 2, 8, 9],
    [-256, -9, -1, 0, 9],
    [-(1 << 31), (1 << 31) - 1],
    [-(1 << 31) - 1, 1 << 31],
    [-huge, 7, huge],
    [],
]:
    direct: list[int] = []
    for digit in source:
        direct.append(sum(divmod(2 * digit, 10)))
    show(direct)
    show([sum(divmod(2 * digit, 10)) for digit in source])

# Aliasing the output with the source must retain live-list iteration.
values = [1, 2]
output = values
for digit in values:
    output.append(sum(divmod(2 * digit, 10)))
    if len(values) == 5:
        break
show(values)

# A callback changes a later value to BigInt and extends the source.
values = [1, 2]
output = []
for digit in values:
    output.append(sum(divmod(2 * digit, 10)))
    if len(output) == 1:
        change(values)
show(output)
show(values)

# for captures the original list even when the source name is rebound.
values = [1, 2, 3]
original = values
output = []
for digit in values:
    output.append(sum(divmod(2 * digit, 10)))
    values = [huge]
show(output)
show(original)
show(values)

for stop_early in [True, False]:
    output = []
    digit = 0
    for digit in [1, 2, 3, 4, 5]:
        if digit == 2:
            continue
        output.append(sum(divmod(2 * digit, 10)))
        if stop_early and digit == 4:
            break
    else:
        output.append(100)
    show(output)
    print(digit)

for turn in range(12):
    output = []
    digit = 0
    try:
        for digit in [1, 2, 3, 4]:
            output.append(sum(divmod(2 * digit, 3 - digit)))
    except ZeroDivisionError:
        if turn == 11:
            print("division failed")
            show(output)
            print(digit)

# Manual cursor loops retain ordinary negative-index and bounds behavior.
values = [2, 4, 8]
for initial in [-4, -3, -1, 0, 2, 3, 4, huge, -huge]:
    cursor = initial
    output = []
    try:
        while cursor < len(values):
            output.append(sum(divmod(2 * values[cursor], 10)))
            cursor += 1
        else:  # noqa: PLW0120 - exercise while-else on normal exhaustion
            output.append(100)
    except IndexError:
        print("index failed")
    print(cursor)
    show(output)

# while reevaluates its source binding; an alias can also mutate future items.
values = [1, 2, 3]
cursor = 0
output = []
while cursor < len(values):
    output.append(sum(divmod(2 * values[cursor], 10)))
    if cursor == 0:
        values = [9, 8]
    cursor += 1
show(output)

values = [1, 2]
alias = values
cursor = 0
output = []
while cursor < len(values):
    output.append(sum(divmod(2 * values[cursor], 10)))
    if cursor == 0:
        alias[1] = huge
        alias.append(-7)
    cursor += 1
show(output)
