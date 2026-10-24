def observe(text: str) -> str:
    print("source")
    return text


def start() -> int:
    print("start")
    return 9223372036854775808


def retained(source: str) -> list[str]:
    saved: list[str] = []
    for line in source.splitlines():
        source = "rebound"
        if not line:
            continue
        saved.append(line)
        if line == "stop":
            break
    else:
        saved.append("exhausted")
    return saved


separators = ["\n", "\r", "\r\n", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029", "\x1f"]
for separator in separators:
    text = "λ" + separator + separator + "tail🙂" + separator
    for index, line in enumerate(text.splitlines(), -1):
        print(index)
        print(repr(line))
print(repr(retained("")))
print(repr(retained("\r\nfirst\x85\u2028stop\nignored")))
long_line = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
long_line = long_line + long_line + long_line
print(repr(retained(long_line + "\r\n" + long_line + "\n")))

line = "initial"
for index, line in enumerate(observe("a\r\nb\n").splitlines(), start()):
    print(index)
    print(line)
print(line)
for line in observe("").splitlines():
    print(line)
print(line)

with open("invalid.txt", "wb") as output:
    output.write(b"valid\n\xff")
try:
    with open("invalid.txt", "r", encoding="utf-8") as stream:
        for index, line in enumerate(stream.read().splitlines(), start()):
            print(line)
except UnicodeDecodeError:
    print("caught UnicodeDecodeError")
