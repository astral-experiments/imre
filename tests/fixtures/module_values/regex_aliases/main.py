from re import compile as compile_pattern
from patterns import first_word
from reader import find

NUMBER = compile_pattern("[0-9]+")


def number(text: str) -> str:
    if match := NUMBER.search(text):
        return match.group(-0)
    return "absent"


for text in ["42", "déjà", ""]:
    print(number(text))
    if match := first_word(text):
        print(match.group("word") or "absent")
    if match := find(text):
        print(match.start())
