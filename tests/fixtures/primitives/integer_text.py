def repeated(text: str, count: int) -> str:
    result = ""
    for index in range(count):
        result += text
    return result


for text in [
    "١٢٣",
    "۱۲۳",
    "१२३",
    "１２３",
    "𝟙𝟚𝟛",
    "١_２_𝟛",
    "\u2003-١_２_𝟛\u0085",
    "+٠٠٠",
    repeated("١", 100),
    "-" + repeated("９", 100),
]:
    print(int(text))

for text in [
    "",
    " ",
    "\t",
    "\n",
    "\0",
    "\x1c1",
    "+",
    "-",
    "_",
    "١__٢",
    "١_",
    "_١",
    "١ ٢",
    "  +1x\t",
    "²",
    "½",
    "四",
    "\u0301",
    "\U000e0100",
    "🐍",
    "'",
    '"',
    "\\",
    "\u088f",
    "\U00011de0",
    "\U0001faef",
    repeated("x", 220),
    repeated("é", 220),
    repeated("\n", 220),
    repeated("'", 220),
]:
    try:
        print(int(text))
    except ValueError as error:
        print(error)
