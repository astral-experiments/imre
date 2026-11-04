def ascending(data: list[int], out: list[int], text: str, start: int) -> None:
    counter = start
    old = start
    for position, ch in enumerate(text, 1):
        counter += 2
        old = min(out[-1] + 1, data[0] + position, data[1] + (ch == "z"))
        out.append(old)
        if ch == "!":
            break
    print(counter)
    print(old)


def descending(data: list[int], out: list[int], text: str, start: int) -> None:
    counter = start
    for position, ch in enumerate(text, 1):
        if ch == "z":
            continue
        counter -= 13
        out.append(min(out[-1] + 1, data[0], data[1]))
    print(counter)


def twice(data: list[int], out: list[int], text: str, start: int) -> None:
    counter = start
    for position, ch in enumerate(text, 1):
        counter += 2
        counter += 3
        out.append(min(out[-1] + 1, data[0], data[1] + (ch == "a")))
    print(counter)


def main() -> None:
    for text in ["", "a", "1234567", "12345678", "🌻🐱z!uvwxyz", "zabzzcuéK🐍𐀀"]:
        for start in [-2147483648, 2147483647, 0, -(1 << 79), (1 << 93) + 7]:
            for data in [[1, 2], [-2147483648, 2147483647], [1 << 150, -(1 << 100)], [7]]:
                out = [start]
                try:
                    ascending(data, out, text, start)
                    descending(data, out, text, start)
                    twice(data, out, text, start)
                except IndexError:
                    print("index")
                print(len(out))
                print(out[-1])


if __name__ == "__main__":
    main()
