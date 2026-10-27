import random
import secrets
from secrets import choice


def words(events: list[str]) -> list[str]:
    events.append("evaluated")
    return ["once"]


def bounded() -> None:
    alphabet = "aβ🎉"
    numbers = [1, 2, 4, 8, 16, 32, 64]
    for _ in range(80):
        assert secrets.choice(alphabet) in alphabet
        assert choice(seq=numbers) in numbers
        assert choice(b"\x00\xff") in (0, 255)
        assert choice(("left", "right")) in ("left", "right")
        n = choice(range(17, -14, -5))
        assert -13 <= n <= 17 and (17 - n) % 5 == 0
    print("within bounds")


bounded()
events: list[str] = []
print(choice(words(events)))
print(events)
print(choice("🎉"))
print(choice(b"x"))
print(choice((17,)))
print(choice(range(1 << 100, (1 << 100) + 1)))
empty: list[int] = []
try:
    choice(empty)
except IndexError as error:
    print(str(error))
try:
    choice(())
except IndexError as error:
    print(str(error))
try:
    choice("")
except IndexError as error:
    print(str(error))
try:
    choice(range(3, 1))
except IndexError as error:
    print(str(error))
try:
    choice(range(0, 4, 0))
except ValueError as error:
    print(str(error))
try:
    choice(range(1 << 100))
except OverflowError as error:
    print(str(error))
random.seed(1729)
expected = random.getrandbits(64)
random.seed(1729)
choice("abc")
choice(range(13))
print(random.getrandbits(64) == expected)
