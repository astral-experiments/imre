from base64 import a85encode, b85encode, z85encode

def flag(value: bool) -> bool:
    print(value)
    return value

def buffer() -> bytes:
    print("buffer")
    return b"    x"

print(a85encode(foldspaces=flag(True), b=buffer(), pad=flag(False)))
print(b85encode(pad=flag(True), b=buffer()))
print(z85encode(s=buffer()))

for sample in (b"", b"\0", b"\0\0", b"\0\0\0", b"\0" * 4, b" " * 4,
               b"\xff" * 4, b"abcde", b"abcdef", b"abcdefg"):
    print(a85encode(sample))
    print(b85encode(sample))
    print(z85encode(sample))
