def indexed_count(values: list[int], index: int) -> int:
    return values[index % 7].bit_count()


def replace_index(values: list[int]) -> int:
    values[0] = -(2 ** 130 + 3)
    return 0


def population(value: int | bool) -> int:
    return value.bit_count()


def count_or_width(value: int, divisor: int) -> int:
    try:
        return value.bit_count() // divisor
    except ZeroDivisionError:
        return value.bit_length()


print(population(False), population(True), population(0))
for exponent in [1, 31, 61, 62, 63, 64, 127, 128, 511, 2048]:
    power = 1 << exponent
    for value in [power - 1, power, power + 1, -power - 1, -power, -power + 1]:
        print(population(value))

for value in range(-65, 66):
    print(value.bit_count(), value.bit_length())
for value in range(-(1 << 62), -(1 << 62) + 3):
    print(value.bit_count(), value.bit_length())
for word in [0, -(1 << 62), 1 << 62, -(1 << 127)]:
    print(count_or_width(word, 0), count_or_width(word, 1))

operands = [1 << 127, 1, -2, 7, 0, -9, 1 << 63]
assert indexed_count(operands, 7) == 1
assert operands[-1].bit_length() == 64
assert operands[replace_index(operands)].bit_length() == 131
assert indexed_count(operands, 0) == 3
