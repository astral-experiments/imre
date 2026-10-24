def scan(xs: list[int], ys: list[int]) -> tuple[int, int]:
    cursor = 0
    total = 0
    while cursor < len(xs):
        total += xs[cursor] * 2
        if cursor == 0:
            ys.append(1)
        cursor += 1
    return cursor, total


def main() -> None:
    # All lists of length at most two over {0, 1}, smallest first.
    heaps: list[list[int]] = [[], [0], [1], [0, 0], [0, 1], [1, 0], [1, 1]]
    for i in range(len(heaps)):
        for j in range(-1, len(heaps)):
            xs = heaps[i][:]
            ys = xs if j == -1 else heaps[j][:]
            visits, total = scan(xs, ys)
            print(f"{i} {j} {visits} {total}")
            print(bytes(xs))
            print(bytes(ys))


main()
