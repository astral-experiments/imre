from base64 import b32decode, b32encode, b32hexdecode, b32hexencode
from binascii import Error

payload = b"\x00example\xff"
print(b32decode(b32encode(payload)))
print(b32hexdecode(b32hexencode(payload).lower(), casefold=True))
print(b32decode(b"M1023456", map01=b"L"))
for decode in (b32decode, b32hexdecode):
    try:
        decode(b"ABCDEF==")
    except Error as exc:
        print(str(exc))

class Encoded(str):
    def encode(self, encoding):
        print(encoding)
        return b"MY======"

print(b32decode(Encoded("π")))
