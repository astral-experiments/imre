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
