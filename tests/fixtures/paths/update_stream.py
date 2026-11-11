from pathlib import Path

path = Path("update-stream")
path.write_text("old contents")
with path.open("w+", 32, "utf-8") as stream:
    assert stream.readable() and stream.writable()
    assert stream.read() == ""
    stream.write("first")
    stream.seek(0)
    assert stream.read() == "first"
assert stream.closed

with path.open(mode="r+b") as stream:
    stream.seek(1)
    stream.write(b"oo")
assert path.read_bytes() == b"foost"
