import base64
import binascii

decode = base64.b64decode
print(decode(b"[=="))
try:
    decode(b"[==", validate=True)
except binascii.Error as exc:
    print(str(exc))
