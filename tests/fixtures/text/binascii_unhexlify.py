from binascii import Error, a2b_hex, unhexlify

print((unhexlify(""), unhexlify("00aBff"), a2b_hex(b"00AbFF")))
for data in (b"z", b"0g", b"g0", b"aa bb ", b"\xff\xff"):
    try:
        print(a2b_hex(data))
    except Error as exc:
        print(str(exc))
try:
    unhexlify("é")
except Error:
    print("unexpected binascii.Error")
except ValueError as exc:
    print(str(exc))
try:
    unhexlify(17)
except TypeError as exc:
    print(str(exc))
try:
    a2b_hex(hexstr=b"00")
except TypeError as exc:
    print(str(exc))
try:
    unhexlify()
except TypeError as exc:
    print(str(exc))
