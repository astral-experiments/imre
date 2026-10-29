import fnmatch

name = b"\xff" * (4 * 1024 * 1024)
with open("/dev/stdout", "wb") as ready:
    ready.write(b"ready\n")
with open("/dev/stdin", "rb") as gate:
    gate.read(1)
try:
    print(fnmatch.fnmatchcase(name, b"*"))
except MemoryError:
    print("refused")
print(fnmatch.fnmatchcase(b"\xff", b"?"))
