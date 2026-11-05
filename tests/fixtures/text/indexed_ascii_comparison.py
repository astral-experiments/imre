def source(log: list[int], value: str) -> str:
    log.append(1)
    return value

def index(log: list[int], at: int) -> int:
    log.append(2)
    return at

def check(value: str, at: int) -> None:
    log: list[int] = []
    try:
        print("x" == source(log, value)[index(log, at)])
        print(source(log, value)[index(log, at)] != "\0")
        print(source(log, value)[index(log, at)] == "é")
    except IndexError as error:
        print(str(error))
    print(log)


def boundaries(text: str, other: str) -> None:
    if text and (text[0] == "+" or "-" == text[0]):
        print("sign")
    if text and (text[-1] == "é" or "😀" == text[-1]):
        print("suffix")
    if text and (text[0] == "漢" and text[-1] == "é"):
        print("both")
    if text and text[0] == "\0":
        print("nul")
    try:
        if text and (text[1] == "x" or other[0] == "+"):
            print("other")
    except IndexError as error:
        print(str(error))
    try:
        if text and (text[0] == "+" or other[0] == "-"):
            print("mixed receiver")
    except IndexError as error:
        print(str(error))
    if text and (text[0] == "" or text[0] == "++"):
        print("long literal")


for value in ("", "+", "-", "x", "漢é", "😀", "\0"):
    boundaries(value, "")

check("x", 0)
check("漢éx", -2)
check("é", 0)
check("\0", 0)
check("", 0)
check("x", -2)
check("x", 9223372036854775809)
