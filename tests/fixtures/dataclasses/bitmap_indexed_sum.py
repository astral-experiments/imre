from imre import struct


@struct
class Row:
    x: float

    def weight(self) -> float:
        return -self.x * 0.5 + self.x


def sample(rows: list[Row], n: int, count: int) -> None:
    i = -1
    at = -2
    total = -0.0
    for i in range(count):
        at = (17 * i + 5) % n
        total += rows[at].weight()
    print(i)
    print(at)
    print(total)


def main() -> None:
    rows = [Row(0.1)] * 8193
    other = Row(-0.3)
    for j in range(2000):
        rows[(3 * j) % 4097] = other
    sample(rows, len(rows), 0)
    sample(rows, 1, 4000)
    sample(rows, len(rows), 23001)


main()
