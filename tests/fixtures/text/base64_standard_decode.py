from base64 import standard_b64decode
from binascii import Error

print(standard_b64decode(b"d3d3LnB5dGhvbi5vcmc="))
try:
    standard_b64decode(b"abc")
except Error as exc:
    print(str(exc))
