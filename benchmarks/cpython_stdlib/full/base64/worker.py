"""Run one public base64 wrapper with its actual input type and alternate alphabet."""
import base64
import binascii
import sys

with open(sys.argv[2], "rb") as fp:
    data = fp.read()
with open(sys.argv[4], "rb") as fp:
    alphabet = fp.read()
flag = sys.argv[5] == "1"

try:
    if sys.argv[1] == "encode":
        if sys.argv[3] == "none":
            result = base64.b64encode(data)
        elif sys.argv[3] == "text":
            result = base64.b64encode(data, alphabet.decode())
        else:
            result = base64.b64encode(data, alphabet)
    elif sys.argv[1] == "text":
        text = data.decode()
        if sys.argv[3] == "none":
            result = base64.b64decode(text, validate=flag)
        elif sys.argv[3] == "text":
            result = base64.b64decode(text, alphabet.decode(), flag)
        else:
            result = base64.b64decode(text, alphabet, flag)
    elif sys.argv[3] == "none":
        result = base64.b64decode(data, validate=flag)
    elif sys.argv[3] == "text":
        result = base64.b64decode(data, alphabet.decode(), flag)
    else:
        result = base64.b64decode(data, alphabet, flag)
    print("ok")
    print(repr(result))
except binascii.Error as e:
    print("binascii.Error")
    print(str(e))
except ValueError as e:
    print("ValueError")
    print(str(e))
except TypeError as e:
    print("TypeError")
    print(str(e))
except AssertionError as e:
    print("AssertionError")
    print(str(e))
