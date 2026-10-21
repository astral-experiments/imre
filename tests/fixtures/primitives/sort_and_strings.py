from typing import Callable


def source() -> list[str]:
    print("source expression")
    return ["bb", "a", "cc", "d"]


def pulled(value: str) -> str:
    print(f"pulled {value}")
    return value


def length(value: str) -> int:
    print(f"key {value}")
    return len(value)


def choose_key() -> Callable[[str], int]:
    print("key expression")
    return length


def reverse_order() -> bool:
    print("reverse expression")
    return True


class Mutator:
    values: list[str]

    def __init__(self, values: list[str]) -> None:
        self.values = values

    def key(self, value: str) -> int:
        print(f"mutating key {value}")
        self.values.pop()
        return len(value)


def integer_key(value: str) -> int:
    print(f"integer key {value}")
    return int(value)


for value in sorted((pulled(value) for value in source()), key=choose_key(), reverse=reverse_order()):
    print(f"sorted {value}")


original = ["bbb", "aa", "cc", "d"]
mutator = Mutator(original)
result = sorted(original, key=mutator.key)
print(f"source after keys {len(original)}")
for value in result:
    print(f"snapshot {value}")

try:
    failed = sorted(["2", "bad", "must not run"], key=integer_key)
    print(f"unexpected {len(failed)}")
except ValueError:
    print("key error")

for number in sorted([1267650600228229401496703205376, -4611686018427387905, 0, -1267650600228229401496703205376]):
    print(number)
for flag in sorted([True, False, True], reverse=True):
    print(flag)
for text in sorted(["λ", "A", "🙂", "é"], key=None):
    print(text)
for data in sorted([b"\xff", b"", b"a"]):
    print(data)
empty: list[str] = sorted([])
print(f"empty {len(empty)}")

separator = ":"
print(separator.join((separator := item) for item in ["left", "right"]))
print(f"last separator {separator}")
print("-".join(pulled(item) for item in ["λ", "🙂", ""]))
print("/".join("Aλ🙂"))
print("|".join([]))
print("".join(["", "a", ""]))
try:
    print("/".join(str(int(item)) for item in ["1", "bad", "3"]))
except ValueError:
    print("join error")

print("aaaa".replace("aa", "b"))
print("é🙂".replace("", "|"))
print("".replace("", "insert"))
print("a\x00b\x00".replace("\x00", "λ"))
receiver = "hello"
print(receiver.replace("l", (receiver := "X")))
print(receiver)
