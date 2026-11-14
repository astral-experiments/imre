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

    # A negative inverse stride maps the scattered set bits into each partial period.
    thin = [Reading(-11.0, False)] * 65535
    for j in range(1024):
        thin[(6931 * j + 32768) % len(thin)] = other
    thin[-1] = other
    measure(thin, len(thin), 701, 701 + 32768, -13.0)
    measure(thin, len(thin), 123, 123 + len(thin) + 32768, 5.0)

    # A non-coprime stride visits one residue class. Exercise both a whole
    # orbit and a partial last orbit from a nonzero start.
    orbit = [Reading(-11.0, False)] * 7168
    for j in range(1792):
        orbit[3 * j] = other
    measure(orbit, len(orbit), 37, 37 + 4 * 1024, -13.0)
    measure(orbit, len(orbit), 93, 93 + 9 * 1024 + 57, 5.0)
    measure(orbit, len(orbit), 93, 93 + 9 * 1024 + 57, 0.5)

    # Sparse physical edits can repeat one payload, or contain distinct rows.
    sparse = [Reading(-11.0, False)] * 65535
    for j in range(53):
        sparse[(6931 * j + 32768) % len(sparse)] = other
    measure(sparse, len(sparse), 701, 701 + 32768, -13.0)
    measure(sparse, 65531, 123, 123 + 2 * 65531 + 12345, 5.0)
    measure(sparse, len(sparse), 73, 9001, 0.5)
    sparse[32768] = Reading(19.0, False)
    measure(sparse, len(sparse), 19, 49002, 0.0)
    # This prefix never visits the distinct replacement.
    measure(sparse, len(sparse), 0, 9001, -0.0)


main()
