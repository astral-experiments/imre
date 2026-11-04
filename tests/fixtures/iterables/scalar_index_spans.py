def fill(data: list[int], out: list[int], text: str, start: int) -> None:
    for index, ch in enumerate(text, start):
        ordinal = index
        left = out[-1] + 1
        above = data[ordinal] + 1
        corner = data[ordinal - 1] + (ch != "a")
        out.append(min(left, above, corner))

def changed(data: list[int], out: list[int], text: str) -> None:
    for index, ch in enumerate(text, 1):
        ordinal = index
        if ch == "!":
            ordinal = len(data) + 10
        value = min(out[-1] + 1, data[ordinal] + 1, data[ordinal - 1] + (ch != "a"))
        out.append(value)

def prefix(size: int, keep: int) -> list[int]:
    out = list(range(size))
    for _ in range(size - keep):
        out.pop()
    return out

def report(out: list[int]) -> None:
    print(len(out))
    for item in out:
        print(item)

def run(data: list[int], out: list[int], text: str, start: int) -> None:
    try:
        fill(data, out, text, start)
    except IndexError:
        print("index")
    report(out)

def main() -> None:
    for text in ["", "a", "aaaaaaa", "aaaaaaaa", "ab!ab!ab!", "é🐍空aβ🪴é🐍空a", "x" * 70]:
        for start in [1, 0, -2, 4, 1 << 100]:
            run(list(range(75)), prefix(256, 1), text, start)
            run([1, 2, 3], prefix(256, 2), text, start)
        run([1 << 105, 2], [0], text, 1)
        shared = prefix(256, 12)
        run(shared, shared, text, 1)
        out = prefix(256, 1)
        try:
            changed(list(range(50)), out, text)
        except IndexError:
            print("changed")
        report(out)

main()
