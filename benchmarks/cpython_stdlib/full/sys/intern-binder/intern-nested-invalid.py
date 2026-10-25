import sys

def word(value: str, /) -> str:
    return value

print(sys.intern(word(value="inside")))
