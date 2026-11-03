from imre import struct


@struct
class Reading:
    value: float
    mark: bool


def measure(rows: list[Reading], n: int, start: int, stop: int, initial: float) -> None:
    i = -3
    at = -1
    total = initial
    for i in range(start, stop):
        at = (7 * i + 2) % n
        total += rows[at].value
    print(i)
    print(at)
    print(total)


def main() -> None:
    rows = [Reading(-11.0, False)] * 8192
    other = Reading(3.0, True)
    for j in range(2048):
        rows[2 * j] = other
    measure(rows, len(rows), 31, 31 + 4 * len(rows) + 17, 5.0)
    rows[-1] = other
    measure(rows, len(rows), 57, 57 + 2 * len(rows), -3.0)
    measure(rows, 8190, 29, 25002, 0.5)
    measure(rows, len(rows), 7, 7, -0.0)

    zeros = [Reading(-0.0, False)] * 4096
    marked = Reading(-0.0, True)
    for j in range(1024):
        zeros[2 * j] = marked
    measure(zeros, len(zeros), 0, 8192, -0.0)

    # Each partial permutation visits just the negative-zero row.
    for base, edit, start, stop in [(-0.0, 0.0, 3072, 4096), (0.0, -0.0, 0, 3072)]:
        partial = [Reading(base, False)] * 4096
        replacement = Reading(edit, True)
        for j in range(start, stop):
            partial[(7 * j + 2) % len(partial)] = replacement
        measure(partial, len(partial), 0, 3072, -0.0)

    measure(rows, len(rows), 31, 31 + len(rows) - 1, -11.0)
    measure(rows, len(rows), 23, 23 + 3 * len(rows) - 17, 5.0)


main()
