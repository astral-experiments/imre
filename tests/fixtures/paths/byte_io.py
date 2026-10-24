from pathlib import Path


def remove_owner(items: list[Path]) -> bytes:
    items.pop()
    return b"\r\n\x00\xff"


def fail_before_open() -> bytes:
    raise ValueError("argument")


for spelling in ["a", "/a/b", "//host/e", "dir/..", "a/b.py"]:
    for name in ["λ.py", "..", "a\\b", "x\0y", "/abs", "", ".", "child/", "a/b"]:
        try:
            print(Path(spelling).with_name(name=name))
        except ValueError as error:
            print(str(error))
for spelling in ["", "/", "//"]:
    try:
        Path(spelling).with_name("leaf")
    except ValueError as error:
        print(str(error))

items = [Path("binary-data")]
print(items[0].write_bytes(data=remove_owner(items)))
target = Path("binary-data")
print(target.read_bytes())
with target.open(mode="ab") as stream:
    print(stream.write(b"last"))
with target.open("rb") as reader:
    print(reader.read(4))
    print(reader.read())
try:
    target.write_bytes(fail_before_open())
except ValueError:
    print(target.read_bytes())
try:
    with target.open("xb"):
        print("unexpected exclusive create")
except FileExistsError:
    print("exclusive create kept file")
print(target.write_bytes(b""))
print(target.read_bytes())
with target.open("w", encoding="utf-8") as text:
    print(text.write("λ\r\n\r"))
with target.open(mode="a") as append:
    print(append.write("尾"))
with target.open() as default:
    print(default.read())
with target.open(encoding="utf-8", mode="rt") as explicit:
    print(explicit.read())
try:
    Path("missing").read_bytes()
except FileNotFoundError:
    print("missing read")
try:
    Path("tree").read_bytes()
except IsADirectoryError:
    print("directory read")
try:
    Path("bad\0path").write_bytes(b"x")
except ValueError:
    print("nul write")
try:
    Path("missing/child").open("w")
except FileNotFoundError:
    print("missing parent")
