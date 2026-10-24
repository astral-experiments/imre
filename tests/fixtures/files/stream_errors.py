from gzip import BadGzipFile, open as gzip_open
from zlib import error as DeflateError


for name in ["short.gz", "crc.gz", "invalid-block.gz", "invalid-text.gz", "rows.txt"]:
    text = gzip_open(name, "rt", encoding="utf-8")
    try:
        with text:
            for line in text:
                pass
    except BadGzipFile:
        print("invalid gzip")
    except DeflateError:
        print("invalid deflate stream")
    except EOFError:
        print("truncated")
    except UnicodeDecodeError:
        print("invalid text")
    print(text.closed)

for name, size in [("partial.gz", 131071), ("bulk.gz", 262144)]:
    with gzip_open(name, "rb") as stream:
        for n in [size, 131072]:
            try:
                print(len(stream.read(n)))
            except DeflateError:
                print("invalid deflate stream")

with gzip_open("rows.gz", "rb") as stream:
    try:
        stream.read(999999999999999999999999)
    except OverflowError:
        print("overflow")
    print(stream.read(1) == b"a")
try:
    stream.read(0)
except ValueError:
    print("closed")

with open("rows.txt", "r", encoding="utf-8") as text:
    try:
        for line in text:
            text.close()
    except ValueError:
        print("closed between iterations")
