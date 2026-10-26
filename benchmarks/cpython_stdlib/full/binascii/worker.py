"""One native public call on a real bytes or str input. The host preserves the error class."""
import binascii
import sys

operation = sys.argv[1]
flag = sys.argv[2] == "1"
for path in sys.argv[3:]:
    with open(path, "rb") as f:
        data = f.read()
    try:
        if operation == "encode":
            result = binascii.b2a_base64(data, newline=flag)
        elif operation == "text":
            result = binascii.a2b_base64(data.decode(), strict_mode=flag)
        else:
            result = binascii.a2b_base64(data, strict_mode=flag)
        print("ok")
        print(repr(result))
    except binascii.Error as exc:
        print("binascii.Error")
        print(str(exc))
    except ValueError as exc:
        print("ValueError")
        print(str(exc))
