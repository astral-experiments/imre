def identity(items: list[int]) -> list[int]:
    return items


def target(items: list[int], log: list[int]) -> list[int]:
    log.append(2)
    return items


def index(log: list[int]) -> int:
    log.append(3)
    return 0


def rhs(items: list[int], log: list[int]) -> int:
    log.append(1)
    items.append(8)
    return 7


def replace(items: list[int]) -> int:
    items.pop()
    items.append(3)
    return 5


def record(log: list[int], value: int) -> int:
    log.append(value)
    return value


def main() -> None:
    items = [1]
    log: list[int] = []
    target(items, log)[index(log)] = rhs(items, log)
    print(log[0])
    print(log[1])
    print(log[2])
    target(items, log)[index(log)] += rhs(items, log)
    print(log[3])
    print(log[4])
    print(log[5])
    print(items[0])
    alias = identity(items)
    items = [99]
    alias.append(42)
    print(alias[-1])
    print(items[0])
    big = [100000000000000000000000000000000]
    big[0] += replace(big)
    print(big[0])
    saved = big.pop()
    print(saved)
    print(len(big))
    copies = [saved] * 3
    copies[0] = 0
    print(copies[2])
    copies = copies
    print(copies[-2])
    print(len(3 * [1, 2]))
    print(len([1] * -3))
    log = []
    print(record(log, 1) < record(log, 0) < record(log, 9))
    print(len(log))
    print(record(log, 0) < record(log, 1) < record(log, 9))
    print(len(log))
    print(False and record(log, 99) == 99)
    print(True or record(log, 99) == 99)
    print(len(log))
    print(not False)
    print(int(True))
    print(int(False))
    print(None)
    print("héλ🙂")
    print(len("héλ🙂"))
    print("héλ🙂" == "héλ🙂")
    print("a" != "b")
    print(int(" +12_345 "))
    print(0xffffffffffffffffffffffffffffffff)
    print(0o7777777777777777777777777777777777)
    print(0b1111111111111111111111111111111111111111111111111111111111111111111111)
    print("embedded\x00nul")
    print(r"\ud800")
    print("\\ud800")
    print("\ufffd")


main()
