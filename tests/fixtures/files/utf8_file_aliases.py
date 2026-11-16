from pathlib import Path

path = Path("utf8-alias.txt")
path.write_bytes(b"\xef\xbb\xbf\xc3\xa9\r\n")

with path.open(encoding="utf8") as stream:
    print(stream.encoding)
    print(repr(stream.read()))
with path.open(encoding="utf8.ucs2") as stream:
    print(stream.encoding)
    print(repr(stream.read()))
with open(path, encoding="uTf__8", newline="") as stream:
    print(stream.encoding)
    print(repr(stream.read()))
