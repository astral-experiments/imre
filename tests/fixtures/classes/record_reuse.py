from dataclasses import dataclass


@dataclass
class Pair:
    left: int
    right: int


def rotate(keep: bool) -> None:
    pair = Pair(1208925819614629174706176, 1)
    saved: list[Pair] = []
    for i in range(3):
        if keep:
            saved.append(pair)
        pair = Pair(pair.right, pair.left)
    for old in saved:
        print(f"{old.left} {old.right}")
    for i in range(1):
        try:
            pair = Pair(pair.right, int("invalid"))
        except ValueError:
            print(f"{pair.left} {pair.right}")


rotate(False)
rotate(True)
