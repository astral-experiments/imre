# Run under an address limit: both inputs fit, but their translation is 1 GiB.
def grow(char: str) -> str:
    for i in range(15):
        char = char + char
    return char

text = grow("a")
replacement = grow("z")
try:
    print(len(text.translate({97: replacement})))
except MemoryError as error:
    print(repr(error))
    print(repr(str(error)))
print(len(text))
print(repr("a".translate({97: "ok"})))
