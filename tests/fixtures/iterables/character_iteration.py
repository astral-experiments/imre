def count(left: str, right: str) -> int:
    total = 0
    for i, a in enumerate(left, 1):
        for j, b in enumerate(right, 1):
            if a != b:
                total += i + j + ord(a)
    return total


def escaped(text: str) -> list[str]:
    saved: list[str] = []
    for ch in text:
        alias = ch
        if ch != "🙂":
            saved.append(alias)
    return saved


def replaced(text: str) -> str:
    last = "several characters"
    for last in text:
        if last == "α":
            break
    return last


for sample in ["", "\x00a", "αe\u0301🙂", "🙂🙂"]:
    print(count(sample, "α\x00🙂"))
    print(repr(escaped(sample)))
    print(repr(replaced(sample)))
