def start() -> int:
    print("start")
    return 2**70


def check(text: str, stop: bool) -> None:
    number, field = -7, "before"
    for number, field in enumerate(text.split("|"), start()):
        if "é" in field:
            print(f"{number}:{field}")
            if stop:
                break
    else:
        print(f"exhausted {number}:{field}")
    print(f"last {number}:{field}")


def unfiltered(text: str, needle: str) -> None:
    for i, field in enumerate(text.split("|")):
        if needle in field:
            print(f"{i}:{field}")
    for j, field in enumerate(text.split("|")):
        if "|" in field:
            print(f"{j}:{field}")
        else:
            print(f"other {j}:{field}")


for sample in ["", "no hit", "||", "éé||x|", "|x|é|ü|éé", "un|deux", "é|later"]:
    check(sample, False)
    check(sample, True)
unfiltered("é||", "")
