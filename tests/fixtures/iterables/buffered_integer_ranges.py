def capped(data: list[int], out: list[int], text: str) -> None:
    for ch in text:
        advancing = out[-1] + 1
        upper = data[0] + 3
        cost = data[1] + (1 if ch == "x" else 0)
        out.append(min(advancing, upper, cost))


def uncapped(data: list[int], out: list[int], text: str) -> None:
    for ch in text:
        out.append(out[-1] * out[-1] + data[0] + data[1] + ord(ch))


def report(data: list[int], out: list[int], text: str) -> None:
    try:
        capped(data, out, text)
    except IndexError:
        print("index")
    print(len(out))
    if out:
        print(out[-1])


def choose(data: list[int], out: list[int], text: str, flag: bool, value: int) -> int:
    if flag:
        while flag:
            for i, ch in enumerate(text, 1):
                out.append(min(out[-1] + 1, data[0] + i, data[1] + (ch == "x")))
            value = 42
            break
    else:
        while not flag:
            value = -17
            break
    return value


def main() -> None:
    for data in [[2147483647, 2147483647], [-2147483648, -2147483648], [1 << 100, -(1 << 90)], [3]]:
        for initial in [2147483647, -2147483648, 1 << 94]:
            report(data, [initial], "xxxyxyyx")
    report([1, 2], [], "")
    report([1, 2], [], "a")
    out = [2147483647, 2147483647]
    report(out, out, "xxxxx")
    growing = [2147483647]
    uncapped([1, 2], growing, "zab")
    print(growing[-1])
    for seed in [0, 1 << 100]:
        for flag in [False, True]:
            out = [2]
            print(choose([3, 7], out, "xxabcdಊᵪ", flag, seed))
            print(len(out))
            print(out[-1])


if __name__ == "__main__":
    main()
