import sys


def report(part: str, copies: int) -> str:
    lines: list[str] = []
    for _ in range(copies):
        lines.append(part)
    print("finished collecting")
    return "|".join(lines)


def main() -> None:
    part = "x"
    copies = 3
    if len(sys.argv) > 1:
        while len(part) < 262144:
            part += part
        copies = 1024
    try:
        print(len(report(part, copies)))
    except MemoryError:
        print("join raised MemoryError")


main()
