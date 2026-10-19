SEED = 5
BIG = 1208925819614629174706176
RATIO = 3.5
DATA = b"\x00\xff"
PAIR = (BIG, "pair")


def read() -> int:
    return VALUES[0]


def build() -> list[int]:
    print("build once")
    return [SEED + index for index in range(3)]


VALUES = build()
FIRST = read()


class Counter:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value + SEED


COUNTER = Counter(6)


def big() -> int:
    return PAIR[0] + 1


def ratio() -> float:
    return RATIO


def byte() -> int:
    return DATA[1]
