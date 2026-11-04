from base64 import b32encode, b32decode, b32hexencode, b32hexdecode
from binascii import Error

for sample in (b"", b"a", b"ab", b"abc", b"abcd", b"abcde", b"abcdef", b"\xff" * 12):
    print(b32encode(sample))
    print(b32decode(b32encode(sample)))
    print(b32hexencode(sample))
    print(b32hexdecode(b32hexencode(sample)))

print(b32decode(b"mz======", casefold=True))
print(b32hexdecode(b"co======", casefold=True))
for invalid in (b"A", b"========", b"0=======", b"A=AAAAAA", b"AA====0=", b"aaaaaa=="):
    try:
        print(b32decode(invalid))
    except Error as exc:
        print(str(exc))
