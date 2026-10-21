class Bits:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


def operand(label: str, value: int) -> int:
    print(label)
    return value


def divided(left: int, right: int) -> int:
    quotient, remainder = divmod(
        operand("numerator", left), operand("denominator", right)
    )
    return quotient + remainder


values = [
    -(1 << 130) - 37,
    -(1 << 63),
    -4611686018427387904,
    -3,
    -1,
    0,
    1,
    3,
    4611686018427387903,
    1 << 63,
    (1 << 130) + 37,
]
for left in values:
    print(~left)
    for right in values:
        print(left & right)
        print(left | right)
        print(left ^ right)
        if right:
            quotient, remainder = divmod(left, right)
            print(quotient)
            print(remainder)
            print(quotient * right + remainder == left)
    for count in [0, 1, 2, 5, 30, 61, 62, 63, 64, 65, 100, 257]:
        print(left << count)
        print(left >> count)

value = 65
value <<= 120
value |= 31
value ^= 9
value &= ~4
value >>= 3
print(value)
slots = [1 << 150]
slots[0] >>= 100
slots[0] ^= -7
print(slots[0])


bits = Bits(1 << 140)
bits.value >>= 100
bits.value |= 3
print(bits.value)


quotient, remainder = divmod(operand("left", -29), operand("right", 5))
print(quotient)
print(remainder)
number = 100
quotient, remainder = divmod(number, (number := 7))
print(quotient)
print(remainder)
print(number)
print(operand("shift-left", 3) << operand("shift-right", 4))

for number in values:
    for count in [-1, -(1 << 130)]:
        try:
            print(number << count)
        except ValueError as error:
            print(str(error))
        try:
            print(number >> count)
        except ValueError as error:
            print(str(error))
    print(number >> (1 << 130))
    print(number >> 9223372036854775807)
    if number == 0:
        print(number << (1 << 130))
    else:
        try:
            print(number << (1 << 130))
        except (OverflowError, MemoryError):
            # CPython 3.12 used MemoryError; 3.14 reports OverflowError before
            # allocation. Runtime unit tests check the pinned 3.14 category.
            print("unrepresentable shift")
        try:
            print(number << 9223372036854775807)
        except (OverflowError, MemoryError):
            print("unrepresentable shift")

try:
    quotient, remainder = divmod(operand("numerator", 1 << 120), operand("zero", 0))
except ZeroDivisionError:
    # Builtin zero-division wording differs between CPython 3.12 and 3.14.
    print("zero division")

print(divided(1 << 180, 17))
try:
    print(divided(1 << 180, 0))
except ZeroDivisionError:
    print("scalarized zero division")
