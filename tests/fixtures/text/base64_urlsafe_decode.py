from base64 import urlsafe_b64decode
from binascii import Error

print(urlsafe_b64decode(b"--__"))
try:
    urlsafe_b64decode(b"abc")
except Error as exc:
    print(str(exc))
