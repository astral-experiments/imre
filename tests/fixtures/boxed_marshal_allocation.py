import marshal

document = b"s\x00\x00\x40\x00" + b"x" * (4 * 1024 * 1024)
print("ready", flush=True)
with open("/dev/stdin", "rb") as gate:
    gate.read(1)
try:
    marshal.loads(document)
except MemoryError:
    print("refused")
print(marshal.loads(b"s\x01\x00\x00\x00x") == b"x")
