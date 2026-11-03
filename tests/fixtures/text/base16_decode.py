from base64 import b16decode
from binascii import Error

print(b16decode("00AF"))
print(b16decode(b"00af", casefold=True))
try:
    b16decode(b"00af")
except Error as exc:
    print(str(exc))
