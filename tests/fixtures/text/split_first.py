def first(text: str, separator: str) -> str:
    return text.split(separator, 1)[0]


for text in ["", "alpha", "🙂🙂alphaα🙂end", "α🙂α🙂"]:
    for separator in [".", "🙂", "α🙂"]:
        print(repr(first(text, separator)))

source = "independent " + str(42)
prefix = first(source, "?")
source += " changed"
print(prefix)
print(source)

try:
    print(first("", ""))
except ValueError as error:
    print(str(error))
