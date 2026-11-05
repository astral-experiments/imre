from re import findall, split


def literals(text: str):
    print(findall(r"ERROR|WARN|INFO", text))
    print(findall(r"ab|a", text))
    print(findall(r"a|ab", text))
    print(split(r"aba|ba", text))
    print(findall(r"(ab|bc|c|dx|ef)", text))
    print(findall(r"é|λx|👋", text))
    print(findall(r"(?:ab|b){2}", text))


def binary(data: bytes):
    print(findall(rb"\xff|\x80x|ab", data))
    print(split(rb"\xff|ab", data))


for value in ["", "abababa", "bcadxcef", "ERROR WARN INFOERR", "éλx👋éλ", "abbabb"]:
    literals(value)
for data in [b"", b"ab\xffab\x80x!", b"\x80\xff"]:
    binary(data)
