from imre import struct


@struct
class Row:
    value: float
    marked: bool


def write(rows: list[Row], row: Row, start: int, stop: int, stride: int, offset: int, modulus: int) -> None:
    i = -1
    at = -2
    for i in range(start, stop):
        at = (i * stride + offset) % modulus
        rows[at] = row
    else:
        print(i)
        print(at)


def main() -> None:
    base = Row(-0.0, False)
    alternate = Row(2.0, True)
    rows = [base] * 8192
    alias = rows
    write(rows, alternate, 0, 3081, 127, 23, 8192)
    write(rows, base, 3, 1903, 17, 9, 8187)
    write(rows, alternate, 5, 16389, 128, 11, 8192)
    write(rows, base, 3, 9361, 512, 71, 7680)
    write(rows, alternate, 5, 14013, 2, 61, 8189)
    write(rows, base, 7, 5102, -4, 3, 8179)
    write(rows, Row(5.0, False), 8, 71, 0, 713, 8192)
    write(rows, base, 42, 11, 3, 0, 8192)
    total = 0.0
    marked = 0
    for j in range(len(alias)):
        total += alias[j].value * float(j + 1)
        if alias[j].marked:
            marked += 1
    print(total)
    print(marked)


# A near-complete permutation must retain the original payload in every omitted
# slot. The extra suffix and the noncoprime stride exercise the scalar fallback.
def covering(n: int, length: int, stop: int, stride: int) -> None:
    base = Row(0.0, False)
    alternate = Row(2.0, True)
    saved = Row(-3.0, False)
    rows = [base] * length
    for index in range(0, length, 37):
        rows[index] = saved
    alias = rows
    write(rows, alternate, 3, stop, stride, 11, n)
    checksum = 0.0
    for index in range(length):
        checksum += (index + 1) * alias[index].value
    print(checksum)


main()
covering(1024, 1024, 1020, 37)
covering(1024, 1024, 2051, 1023)
covering(1024, 1031, 1027, 37)
covering(1024, 1024, 1027, 36)
