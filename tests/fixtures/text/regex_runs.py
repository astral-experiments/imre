from re import findall, split


def scan(text: str) -> None:
    print(findall(r"[A-Za-z_]+", text))
    print(split(r"[ ,;]+", text))
    print(findall(r"(?:x|y)+", text))
    print(split(r"x+", text))


for value in ["", "xx", ",;,", " alpha__beta,,x", "éx,y🐍z;", "x\x00y"]:
    scan(value)

text = "a complete field"
fields = split(r",+", text)
print(fields[0] is text)
fields.append("new")
print(split(r",+", text))
