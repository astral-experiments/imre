def fill(data: list[int], out: list[int], text: str) -> None:
    for position, ch in enumerate(text, 1):
        index = ord(ch) - 97
        value = min(out[-1] + 1, data[0] + position, data[index] + 1)
        out.append(value)


def prefix(size: int, keep: int) -> list[int]:
    words = list(range(size))
    for _ in range(size - keep):
        words.pop()
    return words


def run(data: list[int], out: list[int], text: str) -> None:
    try:
        fill(data, out, text)
    except IndexError:
        print("index")
    print(len(out))
    if out:
        print(out[-1])


def read_prefix(data: list[int], out: list[int], text: str) -> None:
    for position, ch in enumerate(text, 1):
        value = min(out[-1] + 1, out[0], data[0] + position)
        out.append(value)


def main() -> None:
    for text in ["", "abcdefgh", "aaaaaaaaz", "aaaaaaaaΣ", "b" * 62, "a" * 63, "a" * 64, "a" * 65]:
        for size in [0, 4, 64, 128]:
            run([11, 3, 8], prefix(size, min(size, 1)), text)
        words = prefix(64, 12)
        run(words, words, text)
        words = prefix(64, 3)
        words[0] = -(1 << 110)
        run([2, 3], words, text)
        print(words[0])
        read_prefix([2], words, text)
        print(words[-1])
        words = prefix(64, 1)
        words[0] = 1 << 110
        run([2, 3], words, text)
        run([1 << 91, 2], prefix(64, 1), text)


if __name__ == "__main__":
    main()
