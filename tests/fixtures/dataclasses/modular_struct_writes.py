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


main()
