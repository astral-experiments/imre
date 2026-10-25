"""Exact string predicate, descriptor identity and argument-evaluation order."""

def receiver(text: str) -> str:
    print("receiver")
    return text


def argument(value: int) -> int:
    print(f"argument {value}")
    if value < 0:
        raise ValueError("argument failed")
    return value


method = str.isalnum
print(method is str.isalnum)
print(method.__name__)
print(method.__qualname__)
print(method.__objclass__ is str)
for text in ["", "abc123", "9_", "α１²Ⅸ", "á", "A𐐀", " 🄇", "🐍"]:
    print(text.isalnum())
    print(method(text))
    print(str.isalnum(text))
try:
    receiver("x").isalnum(argument(0), wrong=argument(1))
except TypeError as error:
    print(error)
try:
    receiver("x").isalnum(argument(0), wrong=argument(-1))
except ValueError as error:
    print(error)
try:
    method()
except TypeError as error:
    print(error)
try:
    str.isalnum(self=argument(1))
except TypeError as error:
    print(error)
try:
    method(argument(0), wrong=argument(1))
except TypeError as error:
    print(error)
try:
    method(argument(0), wrong=argument(-1))
except ValueError as error:
    print(error)
try:
    str.isalnum(b"42")
except TypeError as error:
    print(error)
try:
    str.rstrip([42])
except TypeError as error:
    print(error)
