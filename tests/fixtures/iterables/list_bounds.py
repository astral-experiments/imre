def read(values: list[int], index: int) -> int:
    return sum(divmod(2 * values[index], 10))


def unknown_caller(values: list[int]) -> int:
    return read(values, 0)


def mutate(values: list[int]) -> int:
    values[0] = -(1 << 100)
    return 0


def argument_read(values: list[int], index: int) -> int:
    return sum(divmod(2 * values[index], 10))


def captured_read(values: list[int], index: int) -> int:
    return sum(divmod(2 * values[index], 10))


def changing_loop(values: list[int], other: list[int]) -> int:
    total = 0
    for repeat in range(3):
        total += sum(divmod(2 * values[0], 10))
        values = other
    return total


def bounded_read(text: str, index: int) -> int:
    values = list(map(int, text))
    return sum(divmod(2 * values[index], 10))


huge = 1 << 100

# Every caller contributes to a helper's parameter contract, including unknown
# input flowing through another helper and arbitrary-precision literal storage.
print(read(list(map(int, "123")), 1))
for values in [[huge], [-huge], [1 << 31], [-17]]:
    print(unknown_caller(values))

# Mutation through an alias invalidates the original list's digit bounds.
values = list(map(int, "123"))
alias = values
alias[0] = huge
print(sum(divmod(2 * values[0], 10)))
alias.append(-huge)
print(sum(divmod(2 * values[-1], 10)))

# Later argument effects can change an already captured list; keyword binding
# changes native parameter order without changing source evaluation order.
values = list(map(int, "123"))
print(argument_read(values, mutate(values)))
values = list(map(int, "123"))
print(argument_read(index=mutate(values), values=values))

# A pure rebinding changes the local name, not the already captured receiver.
values = list(map(int, "123"))
print(captured_read(values, (values := [huge])[0] * 0))
print(values[0])
values = list(map(int, "456"))
print(sum(divmod(2 * values[(values := [-huge])[0] * 0], 10)))
print(values[0])

# A fact valid on the first iteration cannot survive a rebinding backedge.
print(changing_loop(list(map(int, "9")), [huge]))
print(changing_loop(list(map(int, "9")), [-huge]))

# Successful digit reads remain bounded; invalid reads must raise before any
# native integer assumptions, including empty lists and huge indices.
for text in ["", "12"]:
    for index in [-huge, -3, -1, 0, 1, 2, huge]:
        try:
            print(bounded_read(text, index))
        except IndexError:
            print("index failed")

# A finally mutation applies on both normal and exceptional exits, and handlers
# must not inherit an interval established only by a prior successful producer.
for fail in [False, True]:
    values = list(map(int, "12"))
    try:
        try:
            if fail:
                print(bounded_read("", 0))
            else:
                print(bounded_read("8", 0))
        finally:
            values[0] = huge
    except IndexError:
        values[1] = -huge
    print(sum(divmod(2 * values[0], 10)))
    print(sum(values))
