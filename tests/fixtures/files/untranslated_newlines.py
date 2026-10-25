from pathlib import Path


with open("input.txt", newline="", encoding="utf-8") as f:
    print(repr(f.readline()))
    print(repr(f.read()))
with Path("input.txt").open("r", newline="") as f:
    print(repr(list(f)))
with open("input.txt", newline=None) as f:
    print(repr(list(f)))
with open("cross-chunk.txt", "w", encoding="utf-8", newline="") as f:
    f.write("a" * 8191 + "\r\né\r\r\nx\nend\r")
with open("cross-chunk.txt", newline="") as f:
    line = f.readline()
    print(len(line))
    print(repr(line[-2:]))
    print(repr(list(f)))
with Path("cross-chunk.txt").open("r", newline="") as f:
    print(repr(f.read()[-12:]))
