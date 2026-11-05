from re import findall, split


def scan(text: str) -> None:
    print(findall(r"[A-Za-z_]+", text))
    print(split(r"[ ,;]+", text))
    print(findall(r"(?:x|y)+", text))
    print(split(r"x+", text))
    print(len(findall(r"[A-Za-z_]+", text)))
    print(len(split(r"[ ,;]+", text)))
    print(len(findall(r"x+", text)))
    print(len(findall(r"xy|x", text)))
    print(len(findall(r"([a-z]+):([0-9]+)", text)))


def unicode_runs(text: str) -> None:
    print(findall(r"(\w+)", text))
    print(findall(r"(é+)", text))
    print(len(findall(r"(\w+)", text)))
    print(len(split(r"\d+", text)))
    print(findall(r"[^,]+", text))
    print(split(r"\d+", text))
    print(split(r"[\x7f-\u0080\U0010ffff]+", text))


def byte_runs(data: bytes) -> None:
    print(len(findall(br"[a-z_]+", data)))
    print(len(split(br"\xff+", data)))
    print(len(split(br"[\x80-\xff]+", data)))
    print(findall(rb"[a-z_]+", data))
    print(findall(rb"([^\x80-\xfe]+)", data))
    print(split(rb"[\x00\xff]+", data))
    print(split(rb"\s+", data))


def byte_identity(data: bytes) -> None:
    first = findall(rb"[^;]+", data)
    again = findall(rb"[^;]+", data)
    print(first[0] is first[1])
    print(first[0] is again[0])


for value in ["", "xx", ",;,", " alpha__beta,,x", "a:74 z:2", "éx,y🐍z;", "x\x00y"]:
    scan(value)

text = "a complete field"
fields = split(r",+", text)
print(fields[0] is text)
fields.append("new")
print(split(r",+", text))

for value in ["", "éé,ß_字👋", "\x7f\u0080x\U0010ffff", "x1٢३y", "\x1c hé!"]:
    unicode_runs(value)


for data in [b"", b"\x00\xff", b"a_9 b\xff\xff\x80\x00z", b"\x1c\r\n"]:
    byte_runs(data)


byte_identity(b"x;x")
byte_identity(b"ab;ab")
byte_identity(b"\xff;\xff")
