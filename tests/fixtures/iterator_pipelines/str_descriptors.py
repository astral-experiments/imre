"""Use the same builtin string descriptor directly and as a saved iterator callback."""


def normalized(names: str) -> set[str]:
    return set(map(str.strip, names.split(",")))


def first(items: list[str]) -> str:
    strip = str.strip
    for word in map(strip, items):
        return word
    return ""


print(sorted(normalized("  md5,sha3, md5\x1f")))
print(first([" \N{LATIN SMALL LETTER E WITH ACUTE} ", "uncalled"]))
print(list(filter(str.strip, ["", "  ", " keep ", "\u00a0"])))
print(list(map(str.isalnum, ["A", ""])))
try:
    print(list(map(str.strip, [12])))
except TypeError as error:
    print(error)
