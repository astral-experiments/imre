from io import BytesIO
from pathlib import Path
from shutil import copyfileobj


src = BytesIO(b"first:payload")
dst = BytesIO(b"PREFIX-OLD")
src.seek(6)
dst.seek(7)
print(copyfileobj(src, dst, length=2))
print(src.tell())
print(dst.tell())
dst.seek(0)
print(dst.read())
print(src.closed)
print(dst.closed)

target = Path("stream-output")
with target.open("wb") as writer:
    with Path("input.txt").open("rb") as reader:
        print(copyfileobj(reader, writer, -1))
        print(reader.closed)
        print(writer.closed)
    print(writer.tell())
    print(target.read_bytes())
    writer.flush()
    print(target.read_bytes())

source = Path("unicode-stream")
source.write_bytes("é\r\n🐍\r尾".encode())
for length in [1, 2, 0, -1]:
    with source.open("r", encoding="utf-8") as text_reader:
        with target.open("w", encoding="utf-8") as text_writer:
            print(copyfileobj(text_reader, text_writer, length))
    print(target.read_bytes())

with source.open("r", encoding="utf-8") as text_reader:
    try:
        copyfileobj(text_reader, dst, 1)
    except TypeError as exc:
        print(str(exc))
with target.open("w", encoding="utf-8") as text_writer:
    print(copyfileobj(BytesIO(b""), text_writer))
    try:
        copyfileobj(BytesIO(b"x"), text_writer)
    except TypeError as exc:
        print(str(exc))
