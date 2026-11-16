from pathlib import Path


def write_variant(p: Path, data: str | bytes | None) -> None:
    try:
        print(p.write_text(data))
    except TypeError as exc:
        print(str(exc))
    try:
        print(p.write_bytes(data))
    except TypeError as exc:
        print(str(exc))
    print(p.read_bytes())


path = Path("codec.txt")
for encoding in ["utf-8", "latin-1", "ascii", "utf-8-sig"]:
    for errors in ["strict", "ignore", "replace", "backslashreplace", "xmlcharrefreplace"]:
        try:
            print(path.write_text("ë\r\n🙂\0ż", encoding, errors, "\r\n"))
            print(path.read_bytes())
        except UnicodeEncodeError as exc:
            print(str(exc))
            print(path.read_bytes())

for data in [b"", b"a\r\nb\rc\n", b"\xef\xbf\xbd\xff\xc2\xe2\x82", b"\xef\xbb\xbfa"]:
    path.write_bytes(data)
    for encoding in ["utf-8", "ascii", "latin-1", "utf-8-sig"]:
        for errors in ["strict", "ignore", "replace", "backslashreplace"]:
            try:
                print(path.read_text(encoding, errors))
                print(path.read_text(encoding, errors, ""))
            except UnicodeDecodeError as exc:
                print(str(exc))

for newline in ["", "\r", "\n", "\r\n"]:
    print(path.write_text("a\r\nb\rc\n", encoding="latin-1", newline=newline))
    print(path.read_bytes())
path.write_bytes(b"original")
missing = Path("missing-codec.txt")
for encoding, errors, newline in [
    ("utf-8\0", "strict", ""),
    ("utf-8", "strict\0", ""),
    ("utf-8", "strict", "\n\0"),
]:
    try:
        path.write_text("replacement", encoding, errors, newline)
    except ValueError as exc:
        print(str(exc))
    assert path.read_bytes() == b"original"
    try:
        missing.read_text(encoding, errors, newline)
    except ValueError as exc:
        print(str(exc))
try:
    path.write_text(b"wrong", encoding="nonexistent", newline="bad")
except TypeError as exc:
    print(str(exc))
    print(path.read_bytes())
try:
    path.write_bytes("wrong")
except TypeError as exc:
    print(str(exc))
    print(path.read_bytes())
try:
    path.write_text("valid", encoding="nonexistent", newline="bad")
except ValueError as exc:
    print(str(exc))
    print(path.read_bytes())
try:
    path.write_text("valid", encoding="nonexistent")
except LookupError as exc:
    print(str(exc))
    print(path.read_bytes())
print(path.write_text("valid", errors="unregistered"))
try:
    path.write_text("ë", encoding="ascii", errors="unregistered")
except LookupError as exc:
    print(str(exc))
    print(path.read_bytes())

try:
    path.write_text("ë", encoding="ascii", errors="x" * 398 + "雪tail")
except LookupError as exc:
    print(str(exc))


write_variant(path, "union string")
write_variant(path, b"union bytes")
write_variant(path, None)
