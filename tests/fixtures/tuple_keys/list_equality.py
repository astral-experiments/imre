def observed(label: str, value: list[tuple[str, int]]) -> list[tuple[str, int]]:
    print(label)
    return value


def main() -> None:
    big = 10_000_000_000_000_000_000
    left = [("é", big), ("a", 1), ("a", 1)]
    right = [("a", 1), ("é", 5_000_000_000_000_000_000 * 2), ("a", 1)]
    print(observed("left", left) == observed("right", right))
    print(sorted(left) == sorted(right))
    print(left != left)
    right.pop()
    print(sorted(left) != sorted(right))
    empty: list[tuple[str, int]] = []
    print(empty == sorted(empty))


main()
