def length(value: int | bool) -> int:
    return value.bit_length()


print((0).bit_length())
print(True.bit_length())
print(False.bit_length())
print(length(True))
print(length(1024))
for exponent in [0, 1, 7, 8, 31, 61, 62, 63, 64, 127, 128, 511, 2048]:
    power = 1 << exponent
    for value in [power - 1, power, power + 1, -power - 1, -power, -power + 1]:
        print(value.bit_length())
