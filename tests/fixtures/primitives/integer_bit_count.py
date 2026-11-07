def population(value: int | bool) -> int:
    return value.bit_count()


print(population(False), population(True), population(0))
for exponent in [1, 31, 61, 62, 63, 64, 127, 128, 511, 2048]:
    power = 1 << exponent
    for value in [power - 1, power, power + 1, -power - 1, -power, -power + 1]:
        print(population(value))

for value in range(-65, 66):
    print(value.bit_count(), value.bit_length())
for value in range(-(1 << 62), -(1 << 62) + 3):
    print(value.bit_count(), value.bit_length())
